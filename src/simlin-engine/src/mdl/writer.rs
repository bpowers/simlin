// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! MDL equation text and sketch writer.
//!
//! Converts `Expr0` AST nodes into Vensim MDL-format equation text and
//! serializes datamodel views to MDL sketch format.
//! The key transformation vs the XMILE printer (`ast::print_eqn`) is
//! converting canonical (underscored, lowercase) identifiers back to
//! MDL-style spaced names and using MDL operator syntax.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use crate::ast::{BinaryOp, Expr0, IndexExpr0, UnaryOp, Visitor};
use crate::builtins::UntypedBuiltinFn;
use crate::common::Result;
use crate::datamodel::view_element::{self, LinkPolarity, LinkShape};
use crate::datamodel::{self, DimensionElements, Equation, GraphicalFunction, View, ViewElement};
use crate::lexer::LexerType;
use unicode_xid::UnicodeXID;

/// Replace underscores with spaces -- the reverse of `space_to_underbar()`.
fn underbar_to_space(name: &str) -> String {
    name.replace('_', " ")
}

fn is_mdl_quoted_ident(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"'
}

/// Check whether an MDL identifier requires quoting after underscore->space conversion.
/// Spaces are allowed in bare MDL names, but characters outside identifier classes
/// (for example `$`, `/`, `|`) must be quoted.
fn needs_mdl_quoting(name: &str) -> bool {
    if name.is_empty() || name != name.trim() {
        return true;
    }

    let mut chars = name.chars();
    match chars.next() {
        None => return true,
        Some(c) if !UnicodeXID::is_xid_start(c) && c != '_' => return true,
        _ => {}
    }

    for c in chars {
        if c == ' ' {
            continue;
        }
        if !UnicodeXID::is_xid_continue(c) && c != '_' {
            return true;
        }
    }

    false
}

fn escape_mdl_quoted_ident(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for c in name.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            _ => escaped.push(c),
        }
    }
    escaped
}

/// Format a canonical identifier for MDL output, preserving spaces and
/// adding quotes when the bare form would not round-trip through MDL parsing.
fn format_mdl_ident(name: &str) -> String {
    let display = underbar_to_space(name);
    if is_mdl_quoted_ident(&display) {
        return display;
    }
    if needs_mdl_quoting(&display) {
        format!("\"{}\"", escape_mdl_quoted_ident(&display))
    } else {
        display
    }
}

/// Arrayed element keys encode multidimensional indices as comma-separated
/// canonical names (for example `c,a,f`). Preserve tuple structure so MDL
/// parsers can split indices, and format each token independently.
fn format_mdl_element_key(element_key: &str) -> String {
    element_key
        .split(',')
        .map(format_mdl_ident)
        .collect::<Vec<_>>()
        .join(",")
}

/// Map zero-argument XMILE builtins that are bare keywords in MDL.
/// In Vensim, these are written without parentheses (e.g. `Time` not `TIME()`).
fn mdl_bare_keyword(xmile_name: &str) -> Option<&'static str> {
    match xmile_name {
        "time" => Some("Time"),
        "dt" | "time_step" => Some("TIME STEP"),
        "starttime" | "initial_time" => Some("INITIAL TIME"),
        "endtime" | "stoptime" | "final_time" => Some("FINAL TIME"),
        _ => None,
    }
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
        // Built-in function names are always plain ASCII identifiers.
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

/// Parenthesize `eqn` when the child's precedence requires it.
///
/// For left children: parenthesize only when the parent has strictly higher
/// precedence (lower-precedence child needs grouping).
///
/// For right children of non-commutative operators (`-`, `/`, `MOD`):
/// also parenthesize when the child has equal precedence, because these
/// operators are not associative -- `a - (b - c)` != `(a - b) - c`.
fn mdl_paren_if_necessary(
    parent: &Expr0,
    child: &Expr0,
    is_right_child: bool,
    eqn: String,
) -> String {
    let needs = match parent {
        Expr0::Const(_, _, _) | Expr0::Var(_, _) => false,
        Expr0::App(_, _) | Expr0::Subscript(_, _, _) => false,
        Expr0::Op1(_, _, _) => matches!(child, Expr0::Op2(_, _, _, _)),
        Expr0::Op2(parent_op, _, _, _) => match child {
            Expr0::Op2(child_op, _, _, _) => {
                let parent_prec = parent_op.precedence();
                let child_prec = child_op.precedence();
                if parent_prec > child_prec {
                    true
                } else if is_right_child && parent_prec == child_prec {
                    matches!(parent_op, BinaryOp::Sub | BinaryOp::Div | BinaryOp::Mod)
                } else {
                    false
                }
            }
            _ => false,
        },
        Expr0::If(_, _, _, _) => false,
    };
    if needs { format!("({eqn})") } else { eqn }
}

/// Returns true when `expr` is a 0-arity builtin call with the given name.
fn is_call(expr: &Expr0, name: &str) -> bool {
    matches!(expr, Expr0::App(UntypedBuiltinFn(f, args), _) if f == name && args.is_empty())
}

/// Returns true when `expr` is a variable reference with the given name.
fn is_var(expr: &Expr0, name: &str) -> bool {
    matches!(expr, Expr0::Var(id, _) if id.as_str() == name)
}

/// Returns true when `expr` is `Const(_, v, _)` with value exactly `v`.
fn is_const(expr: &Expr0, v: f64) -> bool {
    matches!(expr, Expr0::Const(_, n, _) if (*n - v).abs() < f64::EPSILON)
}

/// Structurally compare two Expr0 trees, ignoring source locations.
fn exprs_equal(a: &Expr0, b: &Expr0) -> bool {
    match (a, b) {
        (Expr0::Const(_, av, _), Expr0::Const(_, bv, _)) => (av - bv).abs() < f64::EPSILON,
        (Expr0::Var(aid, _), Expr0::Var(bid, _)) => aid == bid,
        (Expr0::App(UntypedBuiltinFn(af, aa), _), Expr0::App(UntypedBuiltinFn(bf, ba), _)) => {
            af == bf && aa.len() == ba.len() && aa.iter().zip(ba).all(|(x, y)| exprs_equal(x, y))
        }
        (Expr0::Op1(ao, al, _), Expr0::Op1(bo, bl, _)) => ao == bo && exprs_equal(al, bl),
        (Expr0::Op2(ao, al, ar, _), Expr0::Op2(bo, bl, br, _)) => {
            ao == bo && exprs_equal(al, bl) && exprs_equal(ar, br)
        }
        (Expr0::If(ac, at, af, _), Expr0::If(bc, bt, bf, _)) => {
            exprs_equal(ac, bc) && exprs_equal(at, bt) && exprs_equal(af, bf)
        }
        _ => false,
    }
}

// ---- pattern recognizers ----

/// Match RANDOM 0 1: `uniform(0, 1)`.
fn recognize_random_0_1(expr: &Expr0) -> Option<String> {
    if let Expr0::App(UntypedBuiltinFn(f, args), _) = expr
        && f == "uniform"
        && args.len() == 2
        && is_const(&args[0], 0.0)
        && is_const(&args[1], 1.0)
    {
        return Some("RANDOM 0 1()".to_owned());
    }
    None
}

/// Match LOG 2-arg: `ln(x) / ln(base)`.
fn recognize_log_2arg(expr: &Expr0, walk: &mut impl FnMut(&Expr0) -> String) -> Option<String> {
    if let Expr0::Op2(BinaryOp::Div, l, r, _) = expr
        && let Expr0::App(UntypedBuiltinFn(lf, la), _) = l.as_ref()
        && let Expr0::App(UntypedBuiltinFn(rf, ra), _) = r.as_ref()
        && lf == "ln"
        && rf == "ln"
        && la.len() == 1
        && ra.len() == 1
    {
        return Some(format!("LOG({}, {})", walk(&la[0]), walk(&ra[0])));
    }
    None
}

/// Match QUANTUM: `q * int(x / q)` where both occurrences of q are structurally equal.
fn recognize_quantum(expr: &Expr0, walk: &mut impl FnMut(&Expr0) -> String) -> Option<String> {
    if let Expr0::Op2(BinaryOp::Mul, q_outer, int_call, _) = expr
        && let Expr0::App(UntypedBuiltinFn(f, args), _) = int_call.as_ref()
        && f == "int"
        && args.len() == 1
        && let Expr0::Op2(BinaryOp::Div, x, q_inner, _) = &args[0]
        && exprs_equal(q_outer, q_inner)
    {
        return Some(format!("QUANTUM({}, {})", walk(x), walk(q_outer)));
    }
    None
}

/// Match PULSE: `if (time() >= A :AND: time() < A + max(dt(), B)) then 1 else 0`.
fn recognize_pulse(expr: &Expr0, walk: &mut impl FnMut(&Expr0) -> String) -> Option<String> {
    let (cond, t, f) = match_if(expr)?;
    if !is_const(t, 1.0) || !is_const(f, 0.0) {
        return None;
    }
    // cond = And(Gte(time(), A), Lt(time(), Add(A2, max(dt(), B))))
    let (and_l, and_r) = match_binop(cond, BinaryOp::And)?;
    let (gte_l, a1) = match_binop(and_l, BinaryOp::Gte)?;
    if !is_call(gte_l, "time") {
        return None;
    }
    let (lt_l, lt_r) = match_binop(and_r, BinaryOp::Lt)?;
    if !is_call(lt_l, "time") {
        return None;
    }
    // lt_r = Add(A2, max(dt(), B))
    let (a2, max_call) = match_binop(lt_r, BinaryOp::Add)?;
    if !exprs_equal(a1, a2) {
        return None;
    }
    if let Expr0::App(UntypedBuiltinFn(f, args), _) = max_call
        && f == "max"
        && args.len() == 2
        && is_call(&args[0], "dt")
    {
        return Some(format!("PULSE({}, {})", walk(a1), walk(&args[1])));
    }
    None
}

/// Match PULSE TRAIN:
/// `if (time() >= A :AND: time() <= D :AND: (time() - A) MOD C < B) then 1 else 0`.
fn recognize_pulse_train(expr: &Expr0, walk: &mut impl FnMut(&Expr0) -> String) -> Option<String> {
    let (cond, t, f) = match_if(expr)?;
    if !is_const(t, 1.0) || !is_const(f, 0.0) {
        return None;
    }
    // cond = And(And(Gte(time(), A), Lte(time(), D)), Lt(Mod(Sub(time(), A), C), B))
    let (outer_and_l, outer_and_r) = match_binop(cond, BinaryOp::And)?;
    let (inner_and_l, inner_and_r) = match_binop(outer_and_l, BinaryOp::And)?;

    let (gte_l, a1) = match_binop(inner_and_l, BinaryOp::Gte)?;
    if !is_call(gte_l, "time") {
        return None;
    }
    let (lte_l, d) = match_binop(inner_and_r, BinaryOp::Lte)?;
    if !is_call(lte_l, "time") {
        return None;
    }

    // outer_and_r = Lt(Mod(Sub(time(), A), C), B)
    let (mod_expr, b) = match_binop(outer_and_r, BinaryOp::Lt)?;
    let (sub_expr, c) = match_binop(mod_expr, BinaryOp::Mod)?;
    let (sub_l, a2) = match_binop(sub_expr, BinaryOp::Sub)?;
    if !is_call(sub_l, "time") || !exprs_equal(a1, a2) {
        return None;
    }

    Some(format!(
        "PULSE TRAIN({}, {}, {}, {})",
        walk(a1),
        walk(b),
        walk(c),
        walk(d)
    ))
}

/// Match SAMPLE IF TRUE: `if cond then input else previous(self, init)`.
fn recognize_sample_if_true(
    expr: &Expr0,
    walk: &mut impl FnMut(&Expr0) -> String,
) -> Option<String> {
    let (cond, input, else_branch) = match_if(expr)?;
    if let Expr0::App(UntypedBuiltinFn(f, args), _) = else_branch
        && f == "previous"
        && args.len() == 2
        && is_var(&args[0], "self")
    {
        return Some(format!(
            "SAMPLE IF TRUE({}, {}, {})",
            walk(cond),
            walk(input),
            walk(&args[1])
        ));
    }
    None
}

/// Match ALLOCATE BY PRIORITY:
/// `allocate(supply, last_subscript_ident, demand_with_star, priority, width)`.
fn recognize_allocate(expr: &Expr0, walk: &mut impl FnMut(&Expr0) -> String) -> Option<String> {
    if let Expr0::App(UntypedBuiltinFn(f, args), _) = expr {
        if f != "allocate" || args.len() != 5 {
            return None;
        }
        let supply = walk(&args[0]);
        let priority = walk(&args[3]);
        let width = walk(&args[4]);

        // args[1] is the last subscript dimension name (a Var)
        let dim_name = if let Expr0::Var(id, _) = &args[1] {
            format_mdl_ident(id.as_str())
        } else {
            return None;
        };

        // args[2] is the demand variable, possibly with a final `*` subscript
        // that should be replaced with the dimension name
        let demand_str = if let Expr0::Subscript(id, subs, _) = &args[2] {
            let demand_name = format_mdl_ident(id.as_str());
            if subs.is_empty() {
                demand_name
            } else {
                let mut sub_strs: Vec<String> = subs
                    .iter()
                    .map(|s| match s {
                        IndexExpr0::Wildcard(_) => dim_name.clone(),
                        IndexExpr0::Expr(e) => walk(e),
                        other => {
                            let mut v = MdlPrintVisitor;
                            v.walk_index(other)
                        }
                    })
                    .collect();
                if let Some(IndexExpr0::StarRange(_, _)) = subs.last()
                    && let Some(l) = sub_strs.last_mut()
                {
                    *l = dim_name.clone();
                }
                format!("{demand_name}[{}]", sub_strs.join(", "))
            }
        } else {
            walk(&args[2])
        };

        return Some(format!(
            "ALLOCATE BY PRIORITY({demand_str}, {priority}, 0, {width}, {supply})"
        ));
    }
    None
}

/// Match TIME BASE: `t + dt_val * time()`.
fn recognize_time_base(expr: &Expr0, walk: &mut impl FnMut(&Expr0) -> String) -> Option<String> {
    let (add_l, mul_expr) = match_binop(expr, BinaryOp::Add)?;
    let (dt_val, time_call) = match_binop(mul_expr, BinaryOp::Mul)?;
    if !is_call(time_call, "time") {
        return None;
    }
    Some(format!("TIME BASE({}, {})", walk(add_l), walk(dt_val)))
}

/// Match RANDOM POISSON:
/// `poisson(mean / dt(), seed, min, max) * factor + sdev`.
fn recognize_random_poisson(
    expr: &Expr0,
    walk: &mut impl FnMut(&Expr0) -> String,
) -> Option<String> {
    // Outer: Add(Mul(App("poisson", ...), factor), sdev)
    let (mul_expr, sdev) = match_binop(expr, BinaryOp::Add)?;
    let (poisson_call, factor) = match_binop(mul_expr, BinaryOp::Mul)?;
    if let Expr0::App(UntypedBuiltinFn(f, args), _) = poisson_call
        && f == "poisson"
        && args.len() == 4
    {
        let (mean, dt_call) = match_binop(&args[0], BinaryOp::Div)?;
        if !is_call(dt_call, "dt") {
            return None;
        }
        let min = &args[2];
        let max = &args[3];
        let seed = &args[1];
        return Some(format!(
            "RANDOM POISSON({}, {}, {}, {}, {}, {})",
            walk(min),
            walk(max),
            walk(mean),
            walk(sdev),
            walk(factor),
            walk(seed)
        ));
    }
    None
}

// ---- helper matchers ----

fn match_if(expr: &Expr0) -> Option<(&Expr0, &Expr0, &Expr0)> {
    if let Expr0::If(cond, t, f, _) = expr {
        Some((cond, t, f))
    } else {
        None
    }
}

fn match_binop(expr: &Expr0, expected_op: BinaryOp) -> Option<(&Expr0, &Expr0)> {
    if let Expr0::Op2(op, l, r, _) = expr
        && *op == expected_op
    {
        return Some((l, r));
    }
    None
}

/// Try to recognize known XMILE structural expansions and collapse them
/// back to their compact Vensim builtin form.  Returns `None` when no
/// pattern matches, letting the caller fall through to mechanical conversion.
fn recognize_vensim_patterns(
    expr: &Expr0,
    walk: &mut impl FnMut(&Expr0) -> String,
) -> Option<String> {
    // Order matters: check more specific patterns first.
    if let Some(s) = recognize_random_0_1(expr) {
        return Some(s);
    }
    if let Some(s) = recognize_log_2arg(expr, walk) {
        return Some(s);
    }
    if let Some(s) = recognize_quantum(expr, walk) {
        return Some(s);
    }
    if let Some(s) = recognize_pulse_train(expr, walk) {
        return Some(s);
    }
    if let Some(s) = recognize_pulse(expr, walk) {
        return Some(s);
    }
    if let Some(s) = recognize_sample_if_true(expr, walk) {
        return Some(s);
    }
    if let Some(s) = recognize_allocate(expr, walk) {
        return Some(s);
    }
    if let Some(s) = recognize_time_base(expr, walk) {
        return Some(s);
    }
    if let Some(s) = recognize_random_poisson(expr, walk) {
        return Some(s);
    }
    None
}

struct MdlPrintVisitor;

impl Visitor<String> for MdlPrintVisitor {
    fn walk_index(&mut self, expr: &IndexExpr0) -> String {
        match expr {
            IndexExpr0::Wildcard(_) => "*".to_string(),
            IndexExpr0::StarRange(id, _) => {
                format!("*:{}", format_mdl_ident(id.as_str()))
            }
            IndexExpr0::Range(l, r, _) => format!("{}:{}", self.walk(l), self.walk(r)),
            IndexExpr0::DimPosition(n, _) => format!("@{n}"),
            IndexExpr0::Expr(e) => self.walk(e),
        }
    }

    fn walk(&mut self, expr: &Expr0) -> String {
        // Try pattern recognizers first
        if let Some(s) = recognize_vensim_patterns(expr, &mut |e| self.walk(e)) {
            return s;
        }
        match expr {
            Expr0::Const(s, _, _) => s.clone(),
            Expr0::Var(id, _) => format_mdl_ident(id.as_str()),
            Expr0::App(UntypedBuiltinFn(func, args), _) => {
                // In MDL, TIME and DT are bare keywords (no parentheses).
                if args.is_empty()
                    && let Some(kw) = mdl_bare_keyword(func)
                {
                    return kw.to_owned();
                }
                let mdl_name = xmile_to_mdl_function_name(func);
                let converted: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                let reordered = reorder_args(&mdl_name, converted);
                format!("{}({})", mdl_name, reordered.join(", "))
            }
            Expr0::Subscript(id, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk_index(e)).collect();
                format!("{}[{}]", format_mdl_ident(id.as_str()), args.join(", "))
            }
            Expr0::Op1(op, l, _) => match op {
                UnaryOp::Transpose => {
                    let l = self.walk(l);
                    format!("{l}'")
                }
                _ => {
                    let l = mdl_paren_if_necessary(expr, l, false, self.walk(l));
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
                // Vensim uses MODULO(a, b) function form rather than the
                // binary MOD operator that the XMILE equation parser
                // produces.  Emit the function call so the MDL roundtrip
                // re-parses correctly.
                if *op == BinaryOp::Mod {
                    let l = self.walk(l);
                    let r = self.walk(r);
                    return format!("MODULO({l}, {r})");
                }
                let l = mdl_paren_if_necessary(expr, l, false, self.walk(l));
                let r = mdl_paren_if_necessary(expr, r, true, self.walk(r));
                let op_str = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Exp => "^",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                    BinaryOp::Mod => unreachable!(),
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

/// Convert an XMILE equation string to MDL text via Expr0 round-trip.
/// Falls back to the raw string (with underscores->spaces) on parse failure
/// or empty input.
fn equation_to_mdl(xmile_eqn: &str) -> String {
    if xmile_eqn.is_empty() {
        return String::new();
    }
    // Data equation placeholders (GET DIRECT DATA, GET XLS, etc.) are opaque
    // strings that cannot be parsed as Expr0.  Emit them verbatim, stripping
    // the outer braces that the normalizer adds.
    if is_data_equation(xmile_eqn) {
        let stripped = xmile_eqn
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
            .unwrap_or(xmile_eqn);
        return stripped.to_string();
    }
    match Expr0::new(xmile_eqn, LexerType::Equation) {
        Ok(Some(ast)) => expr0_to_mdl(&ast),
        // Fallback for unparseable equations; best-effort space conversion.
        _ => underbar_to_space(xmile_eqn),
    }
}

/// Data equations use `:=` instead of `=`.  Detect by checking if the
/// raw XMILE equation string begins with one of Vensim's data-fetch
/// function tokens (stored as `{GET_...}` after canonicalization).
fn is_data_equation(xmile_eqn: &str) -> bool {
    let s = xmile_eqn.trim_start_matches('{');
    // The normalizer produces space-separated prefixes like "{GET DIRECT DATA(...)}",
    // but some code paths may store underscore-separated forms.  Accept both.
    s.starts_with("GET DIRECT")
        || s.starts_with("GET XLS")
        || s.starts_with("GET VDF")
        || s.starts_with("GET DATA")
        || s.starts_with("GET 123")
        || s.starts_with("GET_DIRECT")
        || s.starts_with("GET_XLS")
        || s.starts_with("GET_VDF")
        || s.starts_with("GET_DATA")
        || s.starts_with("GET_123")
}

/// Write a graphical-function (lookup table) body into `buf`.
///
/// Format: `([(xmin,ymin)-(xmax,ymax)],(x1,y1),(x2,y2),...)`
fn write_lookup(buf: &mut String, gf: &GraphicalFunction) {
    let xs: Vec<f64> = match &gf.x_points {
        Some(pts) => pts.clone(),
        None => {
            let n = gf.y_points.len();
            if n <= 1 {
                vec![gf.x_scale.min]
            } else {
                let step = (gf.x_scale.max - gf.x_scale.min) / (n - 1) as f64;
                (0..n).map(|i| gf.x_scale.min + step * i as f64).collect()
            }
        }
    };

    write!(
        buf,
        "([({},{})-({},{})]",
        format_f64(gf.x_scale.min),
        format_f64(gf.y_scale.min),
        format_f64(gf.x_scale.max),
        format_f64(gf.y_scale.max),
    )
    .unwrap();

    for (x, y) in xs.iter().zip(gf.y_points.iter()) {
        write!(buf, ",({},{})", format_f64(*x), format_f64(*y)).unwrap();
    }
    buf.push(')');
}

/// Format f64 for MDL output: omit trailing `.0` for whole numbers.
fn format_f64(v: f64) -> String {
    if v.is_infinite() {
        if v.is_sign_positive() {
            "1e+38".to_owned()
        } else {
            "-1e+38".to_owned()
        }
    } else if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

/// Write a single MDL variable entry.
///
/// The standard MDL format for a variable entry is:
/// ```text
/// Name=\n\tequation\n\t~\tunits\n\t~\tcomment\n\t|
/// ```
pub fn write_variable_entry(buf: &mut String, var: &datamodel::Variable) {
    match var {
        datamodel::Variable::Stock(s) => {
            write_stock_variable(buf, s);
            return;
        }
        datamodel::Variable::Module(_) => return,
        _ => {}
    }

    let (ident, equation, units, doc, gf, compat) = match var {
        datamodel::Variable::Flow(f) => (
            &f.ident,
            &f.equation,
            &f.units,
            &f.documentation,
            f.gf.as_ref(),
            &f.compat,
        ),
        datamodel::Variable::Aux(a) => (
            &a.ident,
            &a.equation,
            &a.units,
            &a.documentation,
            a.gf.as_ref(),
            &a.compat,
        ),
        _ => unreachable!(),
    };

    let data_source_eqn = compat_get_direct_equation(compat);
    let effective_gf = if data_source_eqn.is_some() { None } else { gf };

    match equation {
        Equation::Scalar(eqn) => {
            let effective_eqn = data_source_eqn
                .clone()
                .unwrap_or_else(|| wrap_active_initial(eqn, compat));
            write_single_entry(buf, ident, &effective_eqn, &[], units, doc, effective_gf);
        }
        Equation::ApplyToAll(dims, eqn) => {
            let dim_names: Vec<&str> = dims.iter().map(|d| d.as_str()).collect();
            let effective_eqn = data_source_eqn
                .clone()
                .unwrap_or_else(|| wrap_active_initial(eqn, compat));
            write_single_entry(
                buf,
                ident,
                &effective_eqn,
                &dim_names,
                units,
                doc,
                effective_gf,
            );
        }
        Equation::Arrayed(dims, elements, default_eq) => {
            write_arrayed_entries(buf, ident, dims, elements, default_eq, units, doc);
        }
    }
}

fn compat_get_direct_equation(compat: &datamodel::Compat) -> Option<String> {
    let ds = compat.data_source.as_ref()?;
    let quote = |s: &str| s.replace('\'', "\\'");
    let eq = match ds.kind {
        datamodel::DataSourceKind::Data => format!(
            "{{GET DIRECT DATA('{}', '{}', '{}', '{}')}}",
            quote(&ds.file),
            quote(&ds.tab_or_delimiter),
            quote(&ds.row_or_col),
            quote(&ds.cell)
        ),
        datamodel::DataSourceKind::Constants => {
            if ds.cell.is_empty() {
                format!(
                    "{{GET DIRECT CONSTANTS('{}', '{}', '{}')}}",
                    quote(&ds.file),
                    quote(&ds.tab_or_delimiter),
                    quote(&ds.row_or_col)
                )
            } else {
                format!(
                    "{{GET DIRECT CONSTANTS('{}', '{}', '{}', '{}')}}",
                    quote(&ds.file),
                    quote(&ds.tab_or_delimiter),
                    quote(&ds.row_or_col),
                    quote(&ds.cell)
                )
            }
        }
        datamodel::DataSourceKind::Lookups => format!(
            "{{GET DIRECT LOOKUPS('{}', '{}', '{}', '{}')}}",
            quote(&ds.file),
            quote(&ds.tab_or_delimiter),
            quote(&ds.row_or_col),
            quote(&ds.cell)
        ),
        datamodel::DataSourceKind::Subscript => format!(
            "{{GET DIRECT SUBSCRIPT('{}', '{}', '{}', '{}', '')}}",
            quote(&ds.file),
            quote(&ds.tab_or_delimiter),
            quote(&ds.row_or_col),
            quote(&ds.cell)
        ),
    };
    Some(eq)
}

/// Reconstruct a stock's INTEG equation from its decomposed fields.
///
/// The datamodel stores stocks with the initial value in `equation` and
/// inflows/outflows as separate string vectors.  The MDL format requires
/// `INTEG(net_flow, initial_value)`.
fn write_stock_variable(buf: &mut String, stock: &datamodel::Stock) {
    let mut net_flow = String::new();
    for (i, inflow) in stock.inflows.iter().enumerate() {
        if i > 0 {
            net_flow.push('+');
        }
        net_flow.push_str(&format_mdl_ident(inflow));
    }
    for outflow in &stock.outflows {
        net_flow.push('-');
        net_flow.push_str(&format_mdl_ident(outflow));
    }
    if net_flow.is_empty() {
        net_flow.push('0');
    }

    match &stock.equation {
        Equation::Scalar(eqn) => write_stock_entry(
            buf,
            &stock.ident,
            &net_flow,
            &equation_to_mdl(eqn),
            &[],
            &stock.units,
            &stock.documentation,
        ),
        Equation::ApplyToAll(dims, eqn) => {
            let dim_names: Vec<&str> = dims.iter().map(|d| d.as_str()).collect();
            write_stock_entry(
                buf,
                &stock.ident,
                &net_flow,
                &equation_to_mdl(eqn),
                &dim_names,
                &stock.units,
                &stock.documentation,
            );
        }
        Equation::Arrayed(dims, elements, default_eq) => {
            write_arrayed_stock_entries(
                buf,
                &stock.ident,
                &net_flow,
                dims,
                elements,
                default_eq,
                &stock.units,
                &stock.documentation,
            );
        }
    }
}

fn normalized_stock_initial(initial: &str) -> String {
    if initial.trim().is_empty() {
        "0".to_owned()
    } else {
        initial.to_owned()
    }
}

fn write_stock_entry(
    buf: &mut String,
    ident: &str,
    net_flow: &str,
    initial: &str,
    dims: &[&str],
    units: &Option<String>,
    doc: &str,
) {
    let name = format_mdl_ident(ident);
    let initial = normalized_stock_initial(initial);

    if dims.is_empty() {
        write!(buf, "{name}=").unwrap();
    } else {
        let dim_strs: Vec<String> = dims.iter().map(|d| format_mdl_ident(d)).collect();
        write!(buf, "{name}[{}]=", dim_strs.join(",")).unwrap();
    }

    buf.push_str("\n\t");
    buf.push_str(&format!("INTEG({net_flow}, {initial})"));
    write_units_and_comment(buf, units, doc);
}

#[allow(clippy::too_many_arguments)]
fn write_arrayed_stock_entries(
    buf: &mut String,
    ident: &str,
    net_flow: &str,
    _dims: &[String],
    elements: &[(String, String, Option<String>, Option<GraphicalFunction>)],
    _default_equation: &Option<String>,
    units: &Option<String>,
    doc: &str,
) {
    let name = format_mdl_ident(ident);
    let last_idx = elements.len().saturating_sub(1);

    for (i, (elem_name, eqn, _comment, _gf)) in elements.iter().enumerate() {
        let elem_display = format_mdl_element_key(elem_name);
        let initial = normalized_stock_initial(&equation_to_mdl(eqn));

        write!(buf, "{name}[{elem_display}]=").unwrap();
        buf.push_str("\n\t");
        buf.push_str(&format!("INTEG({net_flow}, {initial})"));

        if i < last_idx {
            buf.push_str("\n\t~~|\n");
        } else {
            write_units_and_comment(buf, units, doc);
        }
    }
}

/// If a variable has ACTIVE INITIAL metadata, wrap the equation.
///
/// The datamodel stores `ACTIVE INITIAL(expr, init)` as:
/// - `equation` = expr (the runtime expression)
/// - `compat.active_initial` = Some(init) (the initial value)
fn wrap_active_initial(eqn: &str, compat: &datamodel::Compat) -> String {
    match &compat.active_initial {
        // Both eqn and init are in XMILE format (underscores).  Wrap with
        // init(...) in XMILE form so the whole thing can be parsed by
        // equation_to_mdl, which will map init -> ACTIVE INITIAL and
        // convert all identifiers to spaced MDL form.
        Some(init) => format!("init({eqn}, {init})"),
        None => eqn.to_owned(),
    }
}

/// Write one MDL entry (scalar or apply-to-all).
fn write_single_entry(
    buf: &mut String,
    ident: &str,
    eqn: &str,
    dims: &[&str],
    units: &Option<String>,
    doc: &str,
    gf: Option<&GraphicalFunction>,
) {
    let name = format_mdl_ident(ident);
    let assign_op = if is_data_equation(eqn) { ":=" } else { "=" };

    if dims.is_empty() {
        write!(buf, "{name}{assign_op}").unwrap();
    } else {
        let dim_strs: Vec<String> = dims.iter().map(|d| format_mdl_ident(d)).collect();
        write!(buf, "{name}[{}]{assign_op}", dim_strs.join(",")).unwrap();
    }

    if let Some(gf) = gf {
        // Lookup table replaces the equation
        buf.push_str("\n\t");
        write_lookup(buf, gf);
    } else {
        let mdl_eqn = equation_to_mdl(eqn);
        buf.push_str("\n\t");
        buf.push_str(&mdl_eqn);
    }

    write_units_and_comment(buf, units, doc);
}

/// Write arrayed (per-element) entries.
///
/// `default_equation` records EXCEPT metadata, but the datamodel may omit
/// excepted elements entirely. Without full dimension membership information at
/// this callsite, emitting `name[Dim...]=default` can incorrectly apply the
/// default to omitted EXCEPT members. To preserve behavior, always emit explicit
/// element entries and leave omitted elements implicit.
fn write_arrayed_entries(
    buf: &mut String,
    ident: &str,
    _dims: &[String],
    elements: &[(String, String, Option<String>, Option<GraphicalFunction>)],
    _default_equation: &Option<String>,
    units: &Option<String>,
    doc: &str,
) {
    let name = format_mdl_ident(ident);
    write_arrayed_element_entries(buf, &name, elements, units, doc);
}

fn write_arrayed_element_entries(
    buf: &mut String,
    name: &str,
    elements: &[(String, String, Option<String>, Option<GraphicalFunction>)],
    units: &Option<String>,
    doc: &str,
) {
    let last_idx = elements.len().saturating_sub(1);
    for (i, (elem_name, eqn, _comment, elem_gf)) in elements.iter().enumerate() {
        let elem_display = format_mdl_element_key(elem_name);
        let assign_op = if is_data_equation(eqn) { ":=" } else { "=" };

        write!(buf, "{name}[{elem_display}]{assign_op}").unwrap();

        if let Some(gf) = elem_gf {
            buf.push_str("\n\t");
            write_lookup(buf, gf);
        } else {
            let mdl_eqn = equation_to_mdl(eqn);
            buf.push_str("\n\t");
            buf.push_str(&mdl_eqn);
        }

        if i < last_idx {
            buf.push_str("\n\t~~|\n");
        } else {
            write_units_and_comment(buf, units, doc);
        }
    }
}

/// Append the `~\tunits\n\t~\tcomment\n\t|` trailer.
fn write_units_and_comment(buf: &mut String, units: &Option<String>, doc: &str) {
    buf.push_str("\n\t~\t");
    if let Some(u) = units {
        buf.push_str(u);
    }
    buf.push_str("\n\t~\t");
    buf.push_str(doc);
    buf.push_str("\n\t|");
}

/// Write a dimension definition in MDL format.
///
/// Named:   `DimName: Elem1, Elem2, Elem3 ~~|`
/// Indexed: `DimName: (1-N) ~~|`
/// Mapped:  `DimName: Elem1, Elem2 -> MappedDim ~~|`
/// Element-mapped: `DimName: A1, A2 -> MappedDim: B2, B1 ~~|`
pub fn write_dimension_def(buf: &mut String, dim: &datamodel::Dimension) {
    let name = format_mdl_ident(&dim.name);
    write!(buf, "{name}:").unwrap();

    match &dim.elements {
        DimensionElements::Named(elems) => {
            buf.push_str("\n\t");
            let elem_strs: Vec<String> = elems.iter().map(|e| format_mdl_ident(e)).collect();
            buf.push_str(&elem_strs.join(", "));
        }
        DimensionElements::Indexed(size) => {
            write!(buf, "\n\t(1-{size})").unwrap();
        }
    }

    if let Some(maps_to) = dim.maps_to() {
        write!(buf, " -> {}", format_mdl_ident(maps_to)).unwrap();
    } else if !dim.mappings.is_empty() {
        // Build a source-position index so element-level mappings emit
        // targets in the same order as the source dimension's elements.
        let source_positions: HashMap<String, usize> = match &dim.elements {
            DimensionElements::Named(elems) => elems
                .iter()
                .enumerate()
                .map(|(i, e)| (e.to_lowercase(), i))
                .collect(),
            DimensionElements::Indexed(_) => HashMap::new(),
        };
        let parts: Vec<String> = dim
            .mappings
            .iter()
            .map(|mapping| {
                if mapping.element_map.is_empty() {
                    format_mdl_ident(&mapping.target)
                } else {
                    // Detect one-to-many mappings (from subdimension
                    // expansion) by checking for duplicate source keys.
                    // MDL positional notation can't represent these, so
                    // fall back to a plain dimension-name mapping. This
                    // loses element-level mapping detail on re-import;
                    // use protobuf serialization for lossless roundtrips.
                    let mut seen_sources = std::collections::HashSet::new();
                    let has_one_to_many = mapping
                        .element_map
                        .iter()
                        .any(|(src, _)| !seen_sources.insert(src.as_str()));
                    if has_one_to_many {
                        format_mdl_ident(&mapping.target)
                    } else {
                        let mut sorted_map = mapping.element_map.clone();
                        sorted_map.sort_by_key(|(src, _)| {
                            source_positions
                                .get(src.as_str())
                                .copied()
                                .unwrap_or(usize::MAX)
                        });
                        let target_elems: Vec<String> = sorted_map
                            .iter()
                            .map(|(_, tgt)| format_mdl_ident(tgt))
                            .collect();
                        format!(
                            "({}: {})",
                            format_mdl_ident(&mapping.target),
                            target_elems.join(", ")
                        )
                    }
                }
            })
            .collect();
        write!(buf, " -> {}", parts.join(", ")).unwrap();
    }

    buf.push_str("\n\t~~|\n");
}

// ---- Sketch element serialization ----

/// Write a type 10 line for an Aux element.
/// Sketch element names use bare `underbar_to_space` (not `format_mdl_ident`)
/// because MDL sketch lines are comma-delimited positional records where
/// quoting is not used.
fn write_aux_element(buf: &mut String, aux: &view_element::Aux) {
    let name = underbar_to_space(&aux.name);
    // shape=8 (has equation), bits=3 (visible, primary)
    write!(
        buf,
        "10,{},{},{},{},40,20,8,3,0,0,-1,0,0,0",
        aux.uid, name, aux.x as i32, aux.y as i32,
    )
    .unwrap();
}

/// Write a type 10 line for a Stock element.  See `write_aux_element` for
/// why sketch names use `underbar_to_space` instead of `format_mdl_ident`.
fn write_stock_element(buf: &mut String, stock: &view_element::Stock) {
    let name = underbar_to_space(&stock.name);
    // shape=3 (box/stock shape), bits=3 (visible, primary)
    write!(
        buf,
        "10,{},{},{},{},40,20,3,3,0,0,0,0,0,0",
        stock.uid, name, stock.x as i32, stock.y as i32,
    )
    .unwrap();
}

/// Allocate non-conflicting valve UIDs for flow elements.
///
/// In MDL, each flow is two sketch elements: a valve (type 11) and an attached
/// variable (type 10).  The valve needs a UID that doesn't collide with any
/// existing element UID.  We find the max UID across all elements and allocate
/// valve UIDs starting from max+1.
fn allocate_valve_uids(elements: &[ViewElement]) -> HashMap<i32, i32> {
    let mut max_uid: i32 = 0;
    for elem in elements {
        let uid = match elem {
            ViewElement::Aux(a) => a.uid,
            ViewElement::Stock(s) => s.uid,
            ViewElement::Flow(f) => f.uid,
            ViewElement::Cloud(c) => c.uid,
            ViewElement::Alias(a) => a.uid,
            ViewElement::Module(m) => m.uid,
            ViewElement::Link(l) => l.uid,
            ViewElement::Group(_) => continue,
        };
        max_uid = max_uid.max(uid);
    }

    let mut valve_uids = HashMap::new();
    let mut next_uid = max_uid + 1;
    for elem in elements {
        if let ViewElement::Flow(f) = elem {
            valve_uids.insert(f.uid, next_uid);
            next_uid += 1;
        }
    }
    valve_uids
}

fn max_sketch_uid(elements: &[ViewElement], valve_uids: &HashMap<i32, i32>) -> i32 {
    let mut max_uid = valve_uids.values().copied().max().unwrap_or(0);
    for elem in elements {
        let uid = match elem {
            ViewElement::Aux(a) => a.uid,
            ViewElement::Stock(s) => s.uid,
            ViewElement::Flow(f) => f.uid,
            ViewElement::Cloud(c) => c.uid,
            ViewElement::Alias(a) => a.uid,
            ViewElement::Module(m) => m.uid,
            ViewElement::Link(l) => l.uid,
            ViewElement::Group(_) => continue,
        };
        max_uid = max_uid.max(uid);
    }
    max_uid
}

/// MDL view titles are written on a single `*<title>` line.
/// Collapse CR/LF runs so untrusted titles cannot break sketch structure.
fn sanitize_view_title_for_mdl(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut prev_was_line_break = false;

    for ch in title.chars() {
        if matches!(ch, '\n' | '\r') {
            if !prev_was_line_break {
                out.push(' ');
                prev_was_line_break = true;
            }
            continue;
        }

        out.push(ch);
        prev_was_line_break = false;
    }

    out
}

/// Write a Flow element as type 11 (valve), type 10 (attached flow variable),
/// and type 1 pipe connectors derived from flow endpoints.
///
/// In MDL, flows are two elements: a valve (type 11) at the flow position
/// and an attached variable (type 10) below it. The valve UID is looked up
/// from the pre-allocated valve_uids map to avoid collisions.
fn write_flow_element(
    buf: &mut String,
    flow: &view_element::Flow,
    valve_uids: &HashMap<i32, i32>,
    next_connector_uid: &mut i32,
) {
    // Sketch names: see `write_aux_element` for why `underbar_to_space` is
    // used here instead of `format_mdl_ident`.
    let name = underbar_to_space(&flow.name);
    let valve_uid = valve_uids.get(&flow.uid).copied().unwrap_or(flow.uid - 1);

    // Type 11 (valve): the valve name in Vensim is typically a numeric
    // placeholder (like "48" or "444"). We use the valve uid as the name.
    write!(
        buf,
        "11,{},{},{},{},6,8,34,3,0,0,1,0,0,0",
        valve_uid, valve_uid, flow.x as i32, flow.y as i32,
    )
    .unwrap();

    // Type 10 (attached flow variable): shape=40 (bit 3 = equation, bit 5 = attached)
    // The variable is positioned slightly below the valve.
    let var_y = flow.y as i32 + 16;
    write!(
        buf,
        "\n10,{},{},{},{},49,8,40,3,0,0,-1,0,0,0",
        flow.uid, name, flow.x as i32, var_y,
    )
    .unwrap();

    write_flow_pipe_connectors(buf, flow, valve_uid, next_connector_uid);
}

fn write_flow_pipe_connectors(
    buf: &mut String,
    flow: &view_element::Flow,
    valve_uid: i32,
    next_connector_uid: &mut i32,
) {
    let write_connector =
        |buf: &mut String, connector_uid: i32, from_uid: i32, to_uid: i32, x: i32, y: i32| {
            write!(
                buf,
                "\n1,{},{},{},0,0,0,0,0,0,0,-1--1--1,,1|({},{})|",
                connector_uid, from_uid, to_uid, x, y,
            )
            .unwrap();
        };

    if let Some(first) = flow.points.first()
        && let Some(endpoint_uid) = first.attached_to_uid
    {
        write_connector(
            buf,
            *next_connector_uid,
            valve_uid,
            endpoint_uid,
            first.x as i32,
            first.y as i32,
        );
        *next_connector_uid += 1;
    }

    for point in flow
        .points
        .iter()
        .skip(1)
        .take(flow.points.len().saturating_sub(2))
    {
        write_connector(
            buf,
            *next_connector_uid,
            valve_uid,
            valve_uid,
            point.x as i32,
            point.y as i32,
        );
        *next_connector_uid += 1;
    }

    if flow.points.len() > 1
        && let Some(last) = flow.points.last()
        && let Some(endpoint_uid) = last.attached_to_uid
    {
        write_connector(
            buf,
            *next_connector_uid,
            valve_uid,
            endpoint_uid,
            last.x as i32,
            last.y as i32,
        );
        *next_connector_uid += 1;
    }
}

/// Write a type 12 line for a Cloud element.
fn write_cloud_element(buf: &mut String, cloud: &view_element::Cloud) {
    // Clouds: text="0", shape=0, bits=3 (visible)
    write!(
        buf,
        "12,{},0,{},{},10,8,0,3,0,0,-1,0,0,0",
        cloud.uid, cloud.x as i32, cloud.y as i32,
    )
    .unwrap();
}

/// Write a type 10 line for an Alias (ghost) element.
fn write_alias_element(
    buf: &mut String,
    alias: &view_element::Alias,
    name_map: &HashMap<i32, &str>,
) {
    // Sketch names: see `write_aux_element` for why `underbar_to_space`.
    let name = name_map
        .get(&alias.alias_of_uid)
        .map(|n| underbar_to_space(n))
        .unwrap_or_default();
    // shape=8, bits=2 (visible but bit 0 unset = ghost)
    write!(
        buf,
        "10,{},{},{},{},40,20,8,2,0,3,-1,0,0,0,128-128-128,0-0-0,|12||128-128-128",
        alias.uid, name, alias.x as i32, alias.y as i32,
    )
    .unwrap();
}

/// Write a type 1 line for a Link (connector) element.
///
/// For arc connectors, we reverse-compute a control point from the stored
/// canvas angle using the endpoints of the connected elements.
fn write_link_element(
    buf: &mut String,
    link: &view_element::Link,
    elem_positions: &HashMap<i32, (i32, i32)>,
    use_lettered_polarity: bool,
) {
    let polarity_val = match link.polarity {
        Some(LinkPolarity::Positive) if use_lettered_polarity => 83, // 'S'
        Some(LinkPolarity::Negative) if use_lettered_polarity => 79, // 'O'
        Some(LinkPolarity::Positive) => 43,                          // '+'
        Some(LinkPolarity::Negative) => 45,                          // '-'
        None => 0,
    };

    let from_pos = elem_positions
        .get(&link.from_uid)
        .copied()
        .unwrap_or((0, 0));
    let to_pos = elem_positions.get(&link.to_uid).copied().unwrap_or((0, 0));

    match &link.shape {
        LinkShape::Straight => {
            write!(
                buf,
                "1,{},{},{},0,0,{},0,0,0,0,-1--1--1,,1|(0,0)|",
                link.uid, link.from_uid, link.to_uid, polarity_val,
            )
            .unwrap();
        }
        LinkShape::Arc(canvas_angle) => {
            let (ctrl_x, ctrl_y) = compute_control_point(from_pos, to_pos, *canvas_angle);
            write!(
                buf,
                "1,{},{},{},0,0,{},0,0,0,0,-1--1--1,,1|({},{})|",
                link.uid, link.from_uid, link.to_uid, polarity_val, ctrl_x, ctrl_y,
            )
            .unwrap();
        }
        LinkShape::MultiPoint(points) => {
            let npoints = points.len();
            write!(
                buf,
                "1,{},{},{},0,0,{},0,0,0,0,-1--1--1,,{}|",
                link.uid, link.from_uid, link.to_uid, polarity_val, npoints,
            )
            .unwrap();
            for pt in points {
                write!(buf, "({},{})|", pt.x as i32, pt.y as i32).unwrap();
            }
        }
    }
}

/// Reverse the `angle_from_points` computation: given two endpoint positions
/// and a canvas-space arc angle, compute the single control point (x, y) that
/// lies on the arc between them.
///
/// For a straight connector the caller should use (0, 0) directly rather
/// than calling this function.
fn compute_control_point(from: (i32, i32), to: (i32, i32), canvas_angle: f64) -> (i32, i32) {
    use std::f64::consts::PI;

    let (fx, fy) = (from.0 as f64, from.1 as f64);
    let (tx, ty) = (to.0 as f64, to.1 as f64);

    // Convert canvas angle to XMILE angle
    let xmile_angle = super::view::processing::canvas_angle_to_xmile(canvas_angle);
    let theta_rad = xmile_angle * PI / 180.0;

    // The tangent direction at the start point
    // XMILE angle is counter-clockwise from x-axis with y-up,
    // but canvas is y-down, so we negate y.
    let tan_x = theta_rad.cos();
    let tan_y = -theta_rad.sin();

    // Vector from start to end
    let dx = tx - fx;
    let dy = ty - fy;
    let dist = (dx * dx + dy * dy).sqrt();
    if dist < 1e-6 {
        return (((fx + tx) / 2.0) as i32, ((fy + ty) / 2.0) as i32);
    }

    // Midpoint of start-end segment
    let mx = (fx + tx) / 2.0;
    let my = (fy + ty) / 2.0;

    // Unit vector along start-end
    let ux = dx / dist;
    let uy = dy / dist;

    // The perpendicular bisector of start-end goes through (mx, my)
    // in direction (-uy, ux).
    //
    // The tangent at start forms an angle with the start-end line.
    // The cross product of (ux,uy) and (tan_x,tan_y) tells us which
    // side the arc bulges toward.
    let cross = ux * tan_y - uy * tan_x;
    let dot = ux * tan_x + uy * tan_y;

    // For nearly-straight lines, return the midpoint
    if cross.abs() < 1e-6 {
        return (mx as i32, my as i32);
    }

    // Half-angle between tangent and chord
    // tan(half_angle) = cross / (1 + dot) (half-angle formula)
    let half_angle = cross.atan2(1.0 + dot);

    // The sagitta (distance from midpoint to the arc along the perpendicular bisector):
    // sagitta = (dist/2) * tan(half_angle)
    let sagitta = (dist / 2.0) * half_angle.tan();

    // Control point along the perpendicular bisector
    let perp_x = -uy;
    let perp_y = ux;
    let cx = mx + sagitta * perp_x;
    let cy = my + sagitta * perp_y;

    (cx.round() as i32, cy.round() as i32)
}

/// Stateful writer that accumulates the full MDL file text.
pub struct MdlWriter {
    buf: String,
}

impl Default for MdlWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl MdlWriter {
    pub fn new() -> Self {
        MdlWriter { buf: String::new() }
    }

    /// Orchestrate the full MDL file assembly and return the result.
    pub(super) fn write_project(mut self, project: &datamodel::Project) -> Result<String> {
        self.buf.push_str("{UTF-8}\n");
        let model = &project.models[0];
        self.write_equations_section(model, project);
        self.write_sketch_section(&model.views);
        self.write_settings_section(project);
        Ok(self.buf)
    }

    /// Write sim spec control variables (INITIAL TIME, FINAL TIME, TIME STEP, SAVEPER).
    fn write_sim_specs(&mut self, sim_specs: &datamodel::SimSpecs) {
        let units = sim_specs.time_units.as_deref().unwrap_or("");

        // INITIAL TIME
        write!(
            self.buf,
            "\nINITIAL TIME  = \n\t{}\n\t~\t{}\n\t~\tThe initial time for the simulation.\n\t|\n",
            format_f64(sim_specs.start),
            units,
        )
        .unwrap();

        // FINAL TIME
        write!(
            self.buf,
            "\nFINAL TIME  = \n\t{}\n\t~\t{}\n\t~\tThe final time for the simulation.\n\t|\n",
            format_f64(sim_specs.stop),
            units,
        )
        .unwrap();

        // TIME STEP
        let dt_value = match &sim_specs.dt {
            datamodel::Dt::Dt(v) => format_f64(*v),
            datamodel::Dt::Reciprocal(v) => format!("1/{}", format_f64(*v)),
        };
        let units_with_range = if units.is_empty() {
            "[0,?]".to_owned()
        } else {
            format!("{units} [0,?]")
        };
        write!(
            self.buf,
            "\nTIME STEP  = \n\t{}\n\t~\t{}\n\t~\tThe time step for the simulation.\n\t|\n",
            dt_value, units_with_range,
        )
        .unwrap();

        // SAVEPER
        let saveper_value = match &sim_specs.save_step {
            Some(datamodel::Dt::Dt(v)) => format_f64(*v),
            Some(datamodel::Dt::Reciprocal(v)) => format!("1/{}", format_f64(*v)),
            None => "TIME STEP".to_owned(),
        };
        write!(
            self.buf,
            "\nSAVEPER  = \n\t{}\n\t~\t{}\n\t~\tThe frequency with which output is stored.\n\t|\n",
            saveper_value, units_with_range,
        )
        .unwrap();
    }

    /// Write the full equations section: dimensions, grouped variables, sim specs, terminator.
    fn write_equations_section(&mut self, model: &datamodel::Model, project: &datamodel::Project) {
        // 1. Dimension definitions
        for dim in &project.dimensions {
            write_dimension_def(&mut self.buf, dim);
        }

        // Build a set of variable idents that belong to any group
        let mut grouped_idents: HashSet<&str> = HashSet::new();
        for group in &model.groups {
            for member in &group.members {
                grouped_idents.insert(member.as_str());
            }
        }

        // 2. Variables in group order
        for group in &model.groups {
            // Group marker
            write!(
                self.buf,
                "\n********************************************************\n\t.{}\n********************************************************~\n\t\t{}\n\t|\n",
                // Group names appear in comment-like header blocks, not in equations.
                underbar_to_space(&group.name),
                group.doc.as_deref().unwrap_or(""),
            )
            .unwrap();

            for member_ident in &group.members {
                if let Some(var) = model
                    .variables
                    .iter()
                    .find(|v| v.get_ident() == member_ident)
                {
                    write_variable_entry(&mut self.buf, var);
                    self.buf.push('\n');
                }
            }
        }

        // 3. Ungrouped variables
        for var in &model.variables {
            if !grouped_idents.contains(var.get_ident()) {
                write_variable_entry(&mut self.buf, var);
                self.buf.push('\n');
            }
        }

        // 4. Sim spec variables
        let sim_specs = model.sim_specs.as_ref().unwrap_or(&project.sim_specs);
        self.write_sim_specs(sim_specs);

        // 5. Section terminator
        self.buf
            .push_str("\\\\\\---/// Sketch information - do not modify anything except names\n");
    }

    /// Write the sketch/view section of the MDL file.
    ///
    /// This emits the sketch header, each view's elements, and the sketch
    /// terminator. The section follows the equations terminator line.
    fn write_sketch_section(&mut self, views: &[View]) {
        self.buf
            .push_str("V300  Do not put anything below this section - it will be ignored\n");

        for view in views {
            let View::StockFlow(sf) = view;
            self.write_stock_flow_view(sf);
        }

        self.buf.push_str("///---\\\\\\\n");
    }

    /// Write a single StockFlow view as sketch elements.
    fn write_stock_flow_view(&mut self, sf: &datamodel::StockFlow) {
        let view_title = sanitize_view_title_for_mdl(sf.name.as_deref().unwrap_or("View 1"));
        writeln!(self.buf, "*{}", view_title).unwrap();
        self.buf.push_str(
            "$192-192-192,0,Times New Roman|12||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|96,96,100,0\n",
        );

        // Allocate non-conflicting valve UIDs for flow elements
        let valve_uids = allocate_valve_uids(&sf.elements);
        let mut next_connector_uid = max_sketch_uid(&sf.elements, &valve_uids) + 1;

        // Build position map for link control point computation
        let elem_positions = build_element_positions(&sf.elements, &valve_uids);

        // Build name map for alias resolution
        let name_map = build_name_map(&sf.elements);

        for elem in &sf.elements {
            match elem {
                ViewElement::Aux(aux) => {
                    write_aux_element(&mut self.buf, aux);
                    self.buf.push('\n');
                }
                ViewElement::Stock(stock) => {
                    write_stock_element(&mut self.buf, stock);
                    self.buf.push('\n');
                }
                ViewElement::Flow(flow) => {
                    write_flow_element(&mut self.buf, flow, &valve_uids, &mut next_connector_uid);
                    self.buf.push('\n');
                }
                ViewElement::Link(link) => {
                    write_link_element(
                        &mut self.buf,
                        link,
                        &elem_positions,
                        sf.use_lettered_polarity,
                    );
                    self.buf.push('\n');
                }
                ViewElement::Cloud(cloud) => {
                    write_cloud_element(&mut self.buf, cloud);
                    self.buf.push('\n');
                }
                ViewElement::Alias(alias) => {
                    write_alias_element(&mut self.buf, alias, &name_map);
                    self.buf.push('\n');
                }
                ViewElement::Module(_) | ViewElement::Group(_) => {
                    // Modules and groups are not serialized in MDL sketch format
                }
            }
        }
    }

    /// Write the settings section of the MDL file.
    ///
    /// The settings section follows the sketch terminator (`///---\\\`) and
    /// starts with the `:L<%^E!@` marker. It contains type-coded setting
    /// lines that Vensim reads to restore UI and simulation state.
    fn write_settings_section(&mut self, project: &datamodel::Project) {
        let sim_specs = project
            .models
            .first()
            .and_then(|m| m.sim_specs.as_ref())
            .unwrap_or(&project.sim_specs);

        // The ///---\\\ separator is already emitted by write_sketch_section
        self.buf.push_str(":L<%^E!@\n");

        // Type 22: Unit equivalences
        for unit in &project.units {
            if unit.disabled {
                continue;
            }
            self.buf.push_str("22:");
            if let Some(eq) = &unit.equation {
                write!(self.buf, "{},", eq).unwrap();
            }
            self.buf.push_str(&unit.name);
            for alias in &unit.aliases {
                write!(self.buf, ",{}", alias).unwrap();
            }
            self.buf.push('\n');
        }

        // Type 15: Integration method
        let method_code = match sim_specs.sim_method {
            datamodel::SimMethod::Euler => 0,
            datamodel::SimMethod::RungeKutta4 => 1,
            datamodel::SimMethod::RungeKutta2 => 3,
        };
        writeln!(self.buf, "15:0,0,0,{},0,0", method_code).unwrap();

        // Type 19: Display settings (Vensim default)
        self.buf.push_str("19:100,0\n");
        // Type 27: Font size (Vensim default)
        self.buf.push_str("27:0,\n");
        // Type 34: Optimization settings (Vensim default)
        self.buf.push_str("34:0,\n");
        // Type 4: Time variable name
        self.buf.push_str("4:Time\n");
        // Type 35: Date format name
        self.buf.push_str("35:Date\n");
        // Type 36: Date format pattern
        self.buf.push_str("36:YYYY-MM-DD\n");
        // Type 37-39: Calendar date origin (2000-01-01)
        self.buf.push_str("37:2000\n");
        self.buf.push_str("38:1\n");
        self.buf.push_str("39:1\n");
        // Type 40: Calendar type
        self.buf.push_str("40:2\n");
        // Type 41-42: Calendar sub-settings
        self.buf.push_str("41:0\n");
        self.buf.push_str("42:0\n");

        // Types 24/25/26: Time bounds (initial, final, time step)
        writeln!(self.buf, "24:{}", format_f64(sim_specs.start)).unwrap();
        writeln!(self.buf, "25:{}", format_f64(sim_specs.stop)).unwrap();
        let dt_val = match &sim_specs.dt {
            datamodel::Dt::Dt(v) => format_f64(*v),
            datamodel::Dt::Reciprocal(v) => format!("1/{}", format_f64(*v)),
        };
        writeln!(self.buf, "26:{}", dt_val).unwrap();
    }
}

/// Build a map from element UID to (x, y) position for link control point computation.
///
/// For flow elements, `write_flow_element` emits a synthetic valve using the
/// pre-allocated `valve_uids` map. We register that valve UID here so that any
/// connector whose endpoint is the valve can resolve a position.
fn build_element_positions(
    elements: &[ViewElement],
    valve_uids: &HashMap<i32, i32>,
) -> HashMap<i32, (i32, i32)> {
    let mut positions = HashMap::new();
    for elem in elements {
        let (uid, x, y) = match elem {
            ViewElement::Aux(a) => (a.uid, a.x as i32, a.y as i32),
            ViewElement::Stock(s) => (s.uid, s.x as i32, s.y as i32),
            ViewElement::Flow(f) => {
                // Also register the allocated valve UID so connectors that
                // reference the valve position can resolve.
                if let Some(&valve_uid) = valve_uids.get(&f.uid) {
                    positions.insert(valve_uid, (f.x as i32, f.y as i32));
                }
                (f.uid, f.x as i32, f.y as i32)
            }
            ViewElement::Cloud(c) => (c.uid, c.x as i32, c.y as i32),
            ViewElement::Alias(a) => (a.uid, a.x as i32, a.y as i32),
            ViewElement::Module(m) => (m.uid, m.x as i32, m.y as i32),
            ViewElement::Link(_) | ViewElement::Group(_) => continue,
        };
        positions.insert(uid, (x, y));
    }
    positions
}

/// Build a map from element UID to name for alias (ghost) resolution.
fn build_name_map(elements: &[ViewElement]) -> HashMap<i32, &str> {
    let mut names = HashMap::new();
    for elem in elements {
        match elem {
            ViewElement::Aux(a) => {
                names.insert(a.uid, a.name.as_str());
            }
            ViewElement::Stock(s) => {
                names.insert(s.uid, s.name.as_str());
            }
            ViewElement::Flow(f) => {
                names.insert(f.uid, f.name.as_str());
            }
            _ => {}
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr0, Loc};
    use crate::common::RawIdent;
    use crate::datamodel::{
        Aux, Compat, Equation, Flow, GraphicalFunction, GraphicalFunctionKind,
        GraphicalFunctionScale, SimMethod, Stock, Unit, Variable,
    };
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
    fn variable_references_quote_special_identifiers() {
        let special = Expr0::Var(RawIdent::new_from_str("$_euro"), Loc::default());
        assert_eq!(expr0_to_mdl(&special), "\"$ euro\"");

        let expr = Expr0::Op2(
            BinaryOp::Add,
            Box::new(Expr0::Var(RawIdent::new_from_str("$_euro"), Loc::default())),
            Box::new(Expr0::Var(
                RawIdent::new_from_str("revenue"),
                Loc::default(),
            )),
            Loc::default(),
        );
        assert_eq!(expr0_to_mdl(&expr), "\"$ euro\" + revenue");
    }

    #[test]
    fn quoted_identifiers_escape_embedded_quotes_and_backslashes() {
        assert_eq!(escape_mdl_quoted_ident(r#"it"s"#), r#"it\"s"#);
        assert_eq!(escape_mdl_quoted_ident(r"back\slash"), r"back\\slash");
        assert_eq!(escape_mdl_quoted_ident(r#"a"b\c"#), r#"a\"b\\c"#,);

        assert_eq!(format_mdl_ident(r#"it"s_a_test"#), r#""it\"s a test""#,);
    }

    #[test]
    fn needs_mdl_quoting_edge_cases() {
        assert!(needs_mdl_quoting(""));
        assert!(needs_mdl_quoting(" leading"));
        assert!(needs_mdl_quoting("trailing "));
        assert!(needs_mdl_quoting("1starts_with_digit"));
        assert!(!needs_mdl_quoting("normal name"));
        assert!(!needs_mdl_quoting("_private"));
        assert!(needs_mdl_quoting("has/slash"));
        assert!(needs_mdl_quoting("has|pipe"));
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
    fn right_child_same_precedence_non_commutative() {
        // a - (b - c) must preserve parens: subtraction is not associative
        assert_mdl("a - (b - c)", "a - (b - c)");
        // a / (b / c) must preserve parens: division is not associative
        assert_mdl("a / (b / c)", "a / (b / c)");
        // a - (b + c) must preserve parens: + has same precedence as -
        assert_mdl("a - (b + c)", "a - (b + c)");
    }

    #[test]
    fn left_child_same_precedence_no_extra_parens() {
        // (a - b) - c should NOT get extra parens: left-to-right is natural
        assert_mdl("a - b - c", "a - b - c");
        // (a / b) / c should NOT get extra parens
        assert_mdl("a / b / c", "a / b / c");
        // (a + b) + c should NOT get extra parens: + is commutative anyway
        assert_mdl("a + b + c", "a + b + c");
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
    fn logical_operators_and() {
        // XMILE uses `and` keyword; MDL uses `:AND:` infix operator
        assert_mdl("a and b", "a :AND: b");
    }

    #[test]
    fn logical_operators_or() {
        // XMILE uses `or` keyword; MDL uses `:OR:` infix operator
        assert_mdl("a or b", "a :OR: b");
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
        assert_mdl("uniform(0, 10)", "RANDOM UNIFORM(0, 10)");
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

    // -- pattern recognizer tests (Task 2) --

    #[test]
    fn pattern_random_0_1() {
        // XMILE: uniform(0, 1) -> MDL: RANDOM 0 1()
        assert_mdl("uniform(0, 1)", "RANDOM 0 1()");
    }

    #[test]
    fn pattern_random_0_1_not_matched_different_args() {
        // uniform with non-(0,1) args should NOT match the RANDOM 0 1 pattern
        assert_mdl("uniform(0, 10)", "RANDOM UNIFORM(0, 10)");
    }

    #[test]
    fn pattern_log_2arg() {
        // XMILE: ln(x) / ln(base) -> MDL: LOG(x, base)
        assert_mdl("ln(x) / ln(base)", "LOG(x, base)");
    }

    #[test]
    fn pattern_quantum() {
        // XMILE: q * int(x / q)  ->  MDL: QUANTUM(x, q)
        assert_mdl("q * int(x / q)", "QUANTUM(x, q)");
    }

    #[test]
    fn pattern_quantum_not_matched_different_q() {
        // q1 * int(x / q2) should NOT match QUANTUM when q1 != q2
        assert_mdl("q1 * int(x / q2)", "q1 * INTEGER(x / q2)");
    }

    #[test]
    fn pattern_pulse() {
        // XMILE expansion of PULSE(start, width):
        // IF TIME >= start AND TIME < (start + MAX(DT, width)) THEN 1 ELSE 0
        assert_mdl(
            "if time >= start and time < (start + max(dt, width)) then 1 else 0",
            "PULSE(start, width)",
        );
    }

    #[test]
    fn pattern_pulse_not_matched_missing_lt() {
        // Missing the Lt branch -- should fall through to mechanical conversion
        assert_mdl(
            "if time >= start then 1 else 0",
            "IF THEN ELSE(Time >= start, 1, 0)",
        );
    }

    #[test]
    fn pattern_pulse_train() {
        // XMILE expansion of PULSE TRAIN(start, width, interval, end_val):
        // IF TIME >= start AND TIME <= end_val AND (TIME - start) MOD interval < width THEN 1 ELSE 0
        assert_mdl(
            "if time >= start and time <= end_val and (time - start) mod interval < width then 1 else 0",
            "PULSE TRAIN(start, width, interval, end val)",
        );
    }

    #[test]
    fn mod_emits_modulo() {
        assert_mdl("a mod b", "MODULO(a, b)");
        assert_mdl("(time) mod (5)", "MODULO(Time, 5)");
    }

    #[test]
    fn pattern_sample_if_true() {
        // XMILE expansion of SAMPLE IF TRUE(cond, input, init):
        // IF cond THEN input ELSE PREVIOUS(SELF, init)
        assert_mdl(
            "if cond then input else previous(self, init_val)",
            "SAMPLE IF TRUE(cond, input, init val)",
        );
    }

    #[test]
    fn pattern_time_base() {
        // XMILE expansion of TIME BASE(t, delta):
        // t + delta * TIME
        assert_mdl("t_val + delta * time", "TIME BASE(t val, delta)");
    }

    #[test]
    fn pattern_random_poisson() {
        // XMILE expansion of RANDOM POISSON(min, max, mean, sdev, factor, seed):
        // poisson(mean / dt, seed, min, max) * factor + sdev
        assert_mdl(
            "poisson(mean / dt, seed, min_val, max_val) * factor + sdev",
            "RANDOM POISSON(min val, max val, mean, sdev, factor, seed)",
        );
    }

    #[test]
    fn pattern_fallthrough_no_match() {
        // An If expression that doesn't match any pattern should use mechanical conversion
        assert_mdl("if a > b then c else d", "IF THEN ELSE(a > b, c, d)");
    }

    #[test]
    fn pattern_allocate_by_priority() {
        // XMILE expansion of ALLOCATE BY PRIORITY(demand[region], priority, ignore, width, supply):
        // allocate(supply, region, demand[region.*], priority, width)
        //
        // The last subscript (region.*) is replaced with the dimension name, yielding demand[region].
        // The arguments are reordered: demand first, then priority, then 0 (ignore), width, supply.
        assert_mdl(
            "allocate(supply, region, demand[region.*], priority, width)",
            "ALLOCATE BY PRIORITY(demand[region], priority, 0, width, supply)",
        );
    }

    #[test]
    fn pattern_allocate_by_priority_no_subscript() {
        // When the demand argument has no subscript (simple variable), it passes through as-is.
        assert_mdl(
            "allocate(supply, region, demand, priority, width)",
            "ALLOCATE BY PRIORITY(demand, priority, 0, width, supply)",
        );
    }

    // ---- Task 1: Variable entry formatting (scalar) ----

    fn make_aux(ident: &str, eqn: &str, units: Option<&str>, doc: &str) -> Variable {
        Variable::Aux(Aux {
            ident: ident.to_owned(),
            equation: Equation::Scalar(eqn.to_owned()),
            documentation: doc.to_owned(),
            units: units.map(|u| u.to_owned()),
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        })
    }

    fn make_stock(ident: &str, eqn: &str, units: Option<&str>, doc: &str) -> Variable {
        Variable::Stock(Stock {
            ident: ident.to_owned(),
            equation: Equation::Scalar(eqn.to_owned()),
            documentation: doc.to_owned(),
            units: units.map(|u| u.to_owned()),
            inflows: vec![],
            outflows: vec![],
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        })
    }

    fn make_gf() -> GraphicalFunction {
        GraphicalFunction {
            kind: GraphicalFunctionKind::Continuous,
            x_points: Some(vec![0.0, 1.0, 2.0]),
            y_points: vec![0.0, 0.5, 1.0],
            x_scale: GraphicalFunctionScale { min: 0.0, max: 2.0 },
            y_scale: GraphicalFunctionScale { min: 0.0, max: 1.0 },
        }
    }

    #[test]
    fn scalar_aux_entry() {
        let var = make_aux("characteristic_time", "10", Some("Minutes"), "How long");
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        assert_eq!(
            buf,
            "characteristic time=\n\t10\n\t~\tMinutes\n\t~\tHow long\n\t|"
        );
    }

    #[test]
    fn scalar_aux_entry_quotes_special_identifier_name() {
        let var = make_aux("$_euro", "10", Some("Dmnl"), "");
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        assert_eq!(buf, "\"$ euro\"=\n\t10\n\t~\tDmnl\n\t~\t\n\t|");
    }

    #[test]
    fn scalar_aux_no_units() {
        let var = make_aux("rate", "a + b", None, "");
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        assert_eq!(buf, "rate=\n\ta + b\n\t~\t\n\t~\t\n\t|");
    }

    #[test]
    fn scalar_stock_integ() {
        // Real stocks from the MDL reader store only the initial value in
        // equation, with inflows/outflows in separate fields.  The writer
        // must reconstruct the INTEG(...) expression.
        let var = Variable::Stock(Stock {
            ident: "teacup_temperature".to_owned(),
            equation: Equation::Scalar("180".to_owned()),
            documentation: "Temperature of tea".to_owned(),
            units: Some("Degrees Fahrenheit".to_owned()),
            inflows: vec![],
            outflows: vec!["heat_loss_to_room".to_owned()],
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        assert_eq!(
            buf,
            "teacup temperature=\n\tINTEG(-heat loss to room, 180)\n\t~\tDegrees Fahrenheit\n\t~\tTemperature of tea\n\t|"
        );
    }

    #[test]
    fn stock_with_inflows_and_outflows() {
        let var = Variable::Stock(Stock {
            ident: "population".to_owned(),
            equation: Equation::Scalar("1000".to_owned()),
            documentation: String::new(),
            units: None,
            inflows: vec!["births".to_owned()],
            outflows: vec!["deaths".to_owned()],
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        assert!(
            buf.contains("INTEG(births-deaths, 1000)"),
            "Expected INTEG with both inflow and outflow: {}",
            buf
        );
    }

    #[test]
    fn arrayed_stock_apply_to_all_preserves_initial_value() {
        let var = Variable::Stock(Stock {
            ident: "inventory".to_owned(),
            equation: Equation::ApplyToAll(vec!["region".to_owned()], "100".to_owned()),
            documentation: "Stock by region".to_owned(),
            units: Some("widgets".to_owned()),
            inflows: vec!["inflow".to_owned()],
            outflows: vec!["outflow".to_owned()],
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        assert!(
            buf.contains("inventory[region]=\n\tINTEG(inflow-outflow, 100)"),
            "ApplyToAll stock should emit arrayed INTEG with initial value: {}",
            buf
        );
    }

    #[test]
    fn arrayed_stock_elements_preserve_each_initial_value() {
        let var = Variable::Stock(Stock {
            ident: "inventory".to_owned(),
            equation: Equation::Arrayed(
                vec!["region".to_owned()],
                vec![
                    ("north".to_owned(), "100".to_owned(), None, None),
                    ("south".to_owned(), "200".to_owned(), None, None),
                ],
                None,
            ),
            documentation: "Stock by region".to_owned(),
            units: Some("widgets".to_owned()),
            inflows: vec!["inflow".to_owned()],
            outflows: vec!["outflow".to_owned()],
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        assert!(
            buf.contains("inventory[north]=\n\tINTEG(inflow-outflow, 100)"),
            "First arrayed stock element should retain initial value: {}",
            buf
        );
        assert!(
            buf.contains("inventory[south]=\n\tINTEG(inflow-outflow, 200)"),
            "Second arrayed stock element should retain initial value: {}",
            buf
        );
    }

    #[test]
    fn active_initial_preserved_on_aux() {
        let var = Variable::Aux(Aux {
            ident: "x".to_owned(),
            equation: Equation::Scalar("y * 2".to_owned()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat {
                active_initial: Some("100".to_owned()),
                ..Compat::default()
            },
        });
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        assert!(
            buf.contains("ACTIVE INITIAL(y * 2, 100)"),
            "Expected ACTIVE INITIAL wrapper: {}",
            buf
        );
    }

    #[test]
    fn compat_data_source_reconstructs_get_direct_constants() {
        let var = Variable::Aux(Aux {
            ident: "imported_constants".to_owned(),
            equation: Equation::Scalar("0".to_owned()),
            documentation: String::new(),
            units: None,
            gf: Some(make_gf()),
            ai_state: None,
            uid: None,
            compat: Compat {
                data_source: Some(crate::datamodel::DataSource {
                    kind: crate::datamodel::DataSourceKind::Constants,
                    file: "workbook.xlsx".to_owned(),
                    tab_or_delimiter: "Sheet1".to_owned(),
                    row_or_col: "A".to_owned(),
                    cell: String::new(),
                }),
                ..Compat::default()
            },
        });

        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        assert!(
            buf.contains("imported constants:="),
            "GET DIRECT reconstruction should use := for data equations: {buf}"
        );
        assert!(
            buf.contains("GET DIRECT CONSTANTS('workbook.xlsx', 'Sheet1', 'A')"),
            "writer should reconstruct GET DIRECT CONSTANTS from compat metadata: {buf}"
        );
        assert!(
            !buf.contains("([(0,0)-(2,1)]"),
            "lookup table output must be suppressed when data_source metadata is present: {buf}"
        );
    }

    #[test]
    fn scalar_aux_with_lookup() {
        let gf = make_gf();
        let var = Variable::Aux(Aux {
            ident: "effect_of_x".to_owned(),
            equation: Equation::Scalar("TIME".to_owned()),
            documentation: "Lookup effect".to_owned(),
            units: None,
            gf: Some(gf),
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        assert_eq!(
            buf,
            "effect of x=\n\t([(0,0)-(2,1)],(0,0),(1,0.5),(2,1))\n\t~\t\n\t~\tLookup effect\n\t|"
        );
    }

    #[test]
    fn lookup_without_explicit_x_points() {
        let gf = GraphicalFunction {
            kind: GraphicalFunctionKind::Continuous,
            x_points: None,
            y_points: vec![0.0, 0.5, 1.0],
            x_scale: GraphicalFunctionScale {
                min: 0.0,
                max: 10.0,
            },
            y_scale: GraphicalFunctionScale { min: 0.0, max: 1.0 },
        };
        let var = Variable::Aux(Aux {
            ident: "tbl".to_owned(),
            equation: Equation::Scalar(String::new()),
            documentation: String::new(),
            units: None,
            gf: Some(gf),
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        // x_points auto-generated: 0, 5, 10
        assert_eq!(
            buf,
            "tbl=\n\t([(0,0)-(10,1)],(0,0),(5,0.5),(10,1))\n\t~\t\n\t~\t\n\t|"
        );
    }

    // ---- Task 2: Subscripted equation formatting ----

    #[test]
    fn apply_to_all_entry() {
        let var = Variable::Aux(Aux {
            ident: "rate_a".to_owned(),
            equation: Equation::ApplyToAll(
                vec!["one_dimensional_subscript".to_owned()],
                "100".to_owned(),
            ),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        assert_eq!(
            buf,
            "rate a[one dimensional subscript]=\n\t100\n\t~\t\n\t~\t\n\t|"
        );
    }

    #[test]
    fn apply_to_all_multi_dim() {
        let var = Variable::Aux(Aux {
            ident: "matrix_a".to_owned(),
            equation: Equation::ApplyToAll(
                vec!["dim_a".to_owned(), "dim_b".to_owned()],
                "0".to_owned(),
            ),
            documentation: String::new(),
            units: Some("Dmnl".to_owned()),
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        assert_eq!(buf, "matrix a[dim a,dim b]=\n\t0\n\t~\tDmnl\n\t~\t\n\t|");
    }

    #[test]
    fn arrayed_per_element() {
        let var = Variable::Aux(Aux {
            ident: "rate_a".to_owned(),
            equation: Equation::Arrayed(
                vec!["one_dimensional_subscript".to_owned()],
                vec![
                    ("entry_1".to_owned(), "0.01".to_owned(), None, None),
                    ("entry_2".to_owned(), "0.2".to_owned(), None, None),
                    ("entry_3".to_owned(), "0.3".to_owned(), None, None),
                ],
                None,
            ),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        assert_eq!(
            buf,
            "rate a[entry 1]=\n\t0.01\n\t~~|\nrate a[entry 2]=\n\t0.2\n\t~~|\nrate a[entry 3]=\n\t0.3\n\t~\t\n\t~\t\n\t|"
        );
    }

    #[test]
    fn arrayed_subscript_names_with_underscores() {
        let var = Variable::Aux(Aux {
            ident: "demand".to_owned(),
            equation: Equation::Arrayed(
                vec!["region".to_owned()],
                vec![
                    ("north_america".to_owned(), "100".to_owned(), None, None),
                    ("south_america".to_owned(), "200".to_owned(), None, None),
                ],
                None,
            ),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        // Underscored element names should appear with spaces
        assert!(buf.contains("[north america]"));
        assert!(buf.contains("[south america]"));
    }

    #[test]
    fn arrayed_multidimensional_element_keys_preserve_tuple_shape() {
        let var = Variable::Aux(Aux {
            ident: "power5".to_owned(),
            equation: Equation::Arrayed(
                vec!["subs2".to_owned(), "subs1".to_owned(), "subs3".to_owned()],
                vec![
                    (
                        "c,a,f".to_owned(),
                        "power(var3[subs2, subs1], var4[subs2, subs3])".to_owned(),
                        None,
                        None,
                    ),
                    (
                        "d,b,g".to_owned(),
                        "power(var3[subs2, subs1], var4[subs2, subs3])".to_owned(),
                        None,
                        None,
                    ),
                ],
                None,
            ),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);

        assert!(
            buf.contains("power5[c,a,f]="),
            "missing first tuple key: {buf}"
        );
        assert!(
            buf.contains("power5[d,b,g]="),
            "missing second tuple key: {buf}"
        );
        assert!(
            !buf.contains("power5[\"c,a,f\"]"),
            "tuple key must not be quoted as a single symbol: {buf}"
        );
    }

    #[test]
    fn arrayed_with_per_element_lookup() {
        let gf = make_gf();
        let var = Variable::Aux(Aux {
            ident: "tbl".to_owned(),
            equation: Equation::Arrayed(
                vec!["dim".to_owned()],
                vec![
                    ("a".to_owned(), String::new(), None, Some(gf.clone())),
                    ("b".to_owned(), "5".to_owned(), None, None),
                ],
                None,
            ),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        assert!(buf.contains("tbl[a]=\n\t([(0,0)-(2,1)]"));
        assert!(buf.contains("tbl[b]=\n\t5"));
    }

    // ---- Task 3: Dimension definitions ----

    #[test]
    fn dimension_def_named() {
        let dim = datamodel::Dimension::named(
            "dim_a".to_owned(),
            vec!["a1".to_owned(), "a2".to_owned(), "a3".to_owned()],
        );
        let mut buf = String::new();
        write_dimension_def(&mut buf, &dim);
        assert_eq!(buf, "dim a:\n\ta1, a2, a3\n\t~~|\n");
    }

    #[test]
    fn dimension_def_indexed() {
        let dim = datamodel::Dimension::indexed("dim_b".to_owned(), 5);
        let mut buf = String::new();
        write_dimension_def(&mut buf, &dim);
        assert_eq!(buf, "dim b:\n\t(1-5)\n\t~~|\n");
    }

    #[test]
    fn dimension_def_with_mapping() {
        let mut dim = datamodel::Dimension::named(
            "dim_c".to_owned(),
            vec!["dc1".to_owned(), "dc2".to_owned(), "dc3".to_owned()],
        );
        dim.set_maps_to("dim_b".to_owned());
        let mut buf = String::new();
        write_dimension_def(&mut buf, &dim);
        assert_eq!(buf, "dim c:\n\tdc1, dc2, dc3 -> dim b\n\t~~|\n");
    }

    // ---- Task 3: Data equations ----

    #[test]
    fn data_equation_uses_data_equals() {
        let var = make_aux(
            "direct_data_down",
            "{GET_DIRECT_DATA('data_down.csv',',','A','B2')}",
            None,
            "",
        );
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        // Data equations use := instead of =
        assert!(buf.contains("direct data down:="), "expected := in: {buf}");
    }

    #[test]
    fn non_data_equation_uses_equals() {
        let var = make_aux("x", "42", None, "");
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        assert!(buf.starts_with("x="), "expected = in: {buf}");
    }

    #[test]
    fn is_data_equation_detection() {
        // Underscore-separated form (as might appear in some equation strings)
        assert!(is_data_equation("{GET_DIRECT_DATA('f',',','A','B')}"));
        assert!(is_data_equation("{GET_XLS_DATA('f','s','A','B')}"));
        assert!(is_data_equation("{GET_VDF_DATA('f','v')}"));
        assert!(is_data_equation("{GET_DATA_AT_TIME('v', 5)}"));
        assert!(is_data_equation("{GET_123_DATA('f','s','A','B')}"));

        // Space-separated form (as produced by the normalizer's SymbolClass::GetXls)
        assert!(is_data_equation("{GET DIRECT DATA('f',',','A','B')}"));
        assert!(is_data_equation("{GET XLS DATA('f','s','A','B')}"));
        assert!(is_data_equation("{GET VDF DATA('f','v')}"));
        assert!(is_data_equation("{GET DATA AT TIME('v', 5)}"));
        assert!(is_data_equation("{GET 123 DATA('f','s','A','B')}"));

        assert!(!is_data_equation("100"));
        assert!(!is_data_equation("integ(a, b)"));
        assert!(!is_data_equation(""));
    }

    #[test]
    fn data_equation_preserves_raw_content() {
        // Data equations should not go through expr0_to_mdl() because
        // the GET XLS/DIRECT/etc. placeholders are not parseable as Expr0.
        // Verify the raw content is preserved (not mangled by underbar_to_space).
        let var = make_aux(
            "my_data",
            "{GET DIRECT DATA('data_file.csv',',','A','B2')}",
            None,
            "",
        );
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        // The equation content must preserve underscores in quoted strings
        assert!(
            buf.contains("GET DIRECT DATA('data_file.csv',',','A','B2')"),
            "Data equation content mangled: {}",
            buf
        );
        // Must use := for data equations
        assert!(buf.contains(":="), "Expected := for data equation: {}", buf);
    }

    #[test]
    fn format_f64_whole_numbers() {
        assert_eq!(format_f64(0.0), "0");
        assert_eq!(format_f64(1.0), "1");
        assert_eq!(format_f64(-5.0), "-5");
        assert_eq!(format_f64(100.0), "100");
    }

    #[test]
    fn format_f64_fractional() {
        assert_eq!(format_f64(0.5), "0.5");
        assert_eq!(format_f64(2.71), "2.71");
    }

    #[test]
    fn format_f64_infinity_uses_vensim_numeric_sentinels() {
        assert_eq!(format_f64(f64::INFINITY), "1e+38");
        assert_eq!(format_f64(f64::NEG_INFINITY), "-1e+38");
    }

    #[test]
    fn flow_with_lookup() {
        let gf = make_gf();
        let var = Variable::Flow(Flow {
            ident: "flow_rate".to_owned(),
            equation: Equation::Scalar("TIME".to_owned()),
            documentation: "A flow".to_owned(),
            units: Some("widgets/year".to_owned()),
            gf: Some(gf),
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        assert!(buf.contains("flow rate=\n\t([(0,0)-(2,1)]"));
        assert!(buf.contains("~\twidgets/year"));
        assert!(buf.contains("~\tA flow"));
    }

    #[test]
    fn module_variable_produces_no_output() {
        let var = Variable::Module(datamodel::Module {
            ident: "mod1".to_owned(),
            model_name: "model1".to_owned(),
            documentation: String::new(),
            units: None,
            references: vec![],
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);
        assert!(buf.is_empty());
    }

    // ---- Phase 4 Task 1: Validation ----

    fn make_project(models: Vec<datamodel::Model>) -> datamodel::Project {
        datamodel::Project {
            name: "test".to_owned(),
            sim_specs: datamodel::SimSpecs {
                start: 0.0,
                stop: 100.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: None,
                sim_method: datamodel::SimMethod::Euler,
                time_units: None,
            },
            dimensions: vec![],
            units: vec![],
            models,
            source: None,
            ai_information: None,
        }
    }

    fn make_model(variables: Vec<Variable>) -> datamodel::Model {
        datamodel::Model {
            name: "default".to_owned(),
            sim_specs: None,
            variables,
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }
    }

    #[test]
    fn project_to_mdl_rejects_multiple_models() {
        let project = make_project(vec![make_model(vec![]), make_model(vec![])]);
        let result = crate::mdl::project_to_mdl(&project);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("single model"),
            "error should mention single model, got: {}",
            err
        );
    }

    #[test]
    fn project_to_mdl_rejects_module_variable() {
        let module_var = Variable::Module(datamodel::Module {
            ident: "submodel".to_owned(),
            model_name: "inner".to_owned(),
            documentation: String::new(),
            units: None,
            references: vec![],
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        let project = make_project(vec![make_model(vec![module_var])]);
        let result = crate::mdl::project_to_mdl(&project);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("Module"),
            "error should mention Module, got: {}",
            err
        );
    }

    #[test]
    fn project_to_mdl_succeeds_single_model() {
        let var = make_aux("x", "5", Some("Units"), "A constant");
        let project = make_project(vec![make_model(vec![var])]);
        let result = crate::mdl::project_to_mdl(&project);
        assert!(result.is_ok(), "should succeed: {:?}", result);
        let mdl = result.unwrap();
        assert!(
            mdl.starts_with("{UTF-8}\n"),
            "MDL should start with UTF-8 marker, got: {:?}",
            mdl.lines().next()
        );
        assert!(mdl.contains("x="));
        assert!(mdl.contains("\\\\\\---///"));
    }

    // ---- Phase 4 Task 2: Sim spec emission ----

    #[test]
    fn sim_specs_emission() {
        let sim_specs = datamodel::SimSpecs {
            start: 0.0,
            stop: 100.0,
            dt: datamodel::Dt::Dt(0.5),
            save_step: Some(datamodel::Dt::Dt(1.0)),
            sim_method: datamodel::SimMethod::Euler,
            time_units: Some("Month".to_owned()),
        };
        let mut writer = MdlWriter::new();
        writer.write_sim_specs(&sim_specs);
        let output = writer.buf;

        assert!(
            output.contains("INITIAL TIME  = \n\t0"),
            "should have INITIAL TIME, got: {output}"
        );
        assert!(
            output.contains("~\tMonth\n\t~\tThe initial time for the simulation."),
            "INITIAL TIME should have Month units"
        );
        assert!(
            output.contains("FINAL TIME  = \n\t100"),
            "should have FINAL TIME, got: {output}"
        );
        assert!(
            output.contains("TIME STEP  = \n\t0.5"),
            "should have TIME STEP = 0.5, got: {output}"
        );
        assert!(
            output.contains("Month [0,?]"),
            "TIME STEP should have units with range, got: {output}"
        );
        assert!(
            output.contains("SAVEPER  = \n\t1"),
            "should have SAVEPER = 1, got: {output}"
        );
    }

    #[test]
    fn sim_specs_saveper_defaults_to_time_step() {
        let sim_specs = datamodel::SimSpecs {
            start: 0.0,
            stop: 50.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        };
        let mut writer = MdlWriter::new();
        writer.write_sim_specs(&sim_specs);
        let output = writer.buf;

        assert!(
            output.contains("SAVEPER  = \n\tTIME STEP"),
            "SAVEPER should reference TIME STEP when save_step is None, got: {output}"
        );
    }

    #[test]
    fn sim_specs_reciprocal_dt() {
        let sim_specs = datamodel::SimSpecs {
            start: 0.0,
            stop: 100.0,
            dt: datamodel::Dt::Reciprocal(4.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: Some("Year".to_owned()),
        };
        let mut writer = MdlWriter::new();
        writer.write_sim_specs(&sim_specs);
        let output = writer.buf;

        assert!(
            output.contains("TIME STEP  = \n\t1/4"),
            "reciprocal dt should emit 1/v, got: {output}"
        );
    }

    // ---- Phase 4 Task 3: Equations section assembly ----

    #[test]
    fn equations_section_full_assembly() {
        let var1 = make_aux("growth_rate", "0.05", Some("1/Year"), "Growth rate");
        let var2 = make_stock(
            "population",
            "integ(growth_rate * population, 100)",
            Some("People"),
            "Total population",
        );
        let model = datamodel::Model {
            name: "default".to_owned(),
            sim_specs: None,
            variables: vec![var1, var2],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        };
        let project = datamodel::Project {
            name: "test".to_owned(),
            sim_specs: datamodel::SimSpecs {
                start: 0.0,
                stop: 100.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: None,
                sim_method: datamodel::SimMethod::Euler,
                time_units: Some("Year".to_owned()),
            },
            dimensions: vec![],
            units: vec![],
            models: vec![model],
            source: None,
            ai_information: None,
        };

        let result = crate::mdl::project_to_mdl(&project);
        assert!(result.is_ok(), "should succeed: {:?}", result);
        let mdl = result.unwrap();

        // Variable entries present
        assert!(
            mdl.contains("growth rate="),
            "should contain growth rate variable"
        );
        assert!(
            mdl.contains("population="),
            "should contain population variable"
        );

        // Sim specs present
        assert!(mdl.contains("INITIAL TIME"), "should contain INITIAL TIME");
        assert!(mdl.contains("FINAL TIME"), "should contain FINAL TIME");
        assert!(mdl.contains("TIME STEP"), "should contain TIME STEP");
        assert!(mdl.contains("SAVEPER"), "should contain SAVEPER");

        // Terminator present
        assert!(
            mdl.contains("\\\\\\---/// Sketch information - do not modify anything except names"),
            "should contain section terminator"
        );

        // Ordering: variables before sim specs, sim specs before terminator
        let var_pos = mdl.find("growth rate=").unwrap();
        let initial_pos = mdl.find("INITIAL TIME").unwrap();
        let terminator_pos = mdl.find("\\\\\\---///").unwrap();
        assert!(
            var_pos < initial_pos,
            "variables should come before sim specs"
        );
        assert!(
            initial_pos < terminator_pos,
            "sim specs should come before terminator"
        );
    }

    #[test]
    fn equations_section_with_groups() {
        let var1 = make_aux("rate_a", "10", None, "");
        let var2 = make_aux("rate_b", "20", None, "");
        let var3 = make_aux("ungrouped_var", "30", None, "");
        let group = datamodel::ModelGroup {
            name: "my_group".to_owned(),
            doc: Some("Group docs".to_owned()),
            parent: None,
            members: vec!["rate_a".to_owned(), "rate_b".to_owned()],
            run_enabled: false,
        };
        let model = datamodel::Model {
            name: "default".to_owned(),
            sim_specs: None,
            variables: vec![var1, var2, var3],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![group],
        };
        let project = make_project(vec![model]);

        let result = crate::mdl::project_to_mdl(&project);
        assert!(result.is_ok(), "should succeed: {:?}", result);
        let mdl = result.unwrap();

        // Group marker present
        assert!(
            mdl.contains(".my group"),
            "should contain group marker, got: {mdl}"
        );
        assert!(
            mdl.contains("Group docs"),
            "should contain group documentation"
        );

        // Grouped variables come before ungrouped
        let rate_a_pos = mdl.find("rate a=").unwrap();
        let ungrouped_pos = mdl.find("ungrouped var=").unwrap();
        assert!(
            rate_a_pos < ungrouped_pos,
            "grouped variables should come before ungrouped"
        );
    }

    #[test]
    fn equations_section_with_dimensions() {
        let dim = datamodel::Dimension::named(
            "region".to_owned(),
            vec!["north".to_owned(), "south".to_owned()],
        );
        let var = make_aux("x", "1", None, "");
        let model = make_model(vec![var]);
        let mut project = make_project(vec![model]);
        project.dimensions.push(dim);

        let result = crate::mdl::project_to_mdl(&project);
        assert!(result.is_ok(), "should succeed: {:?}", result);
        let mdl = result.unwrap();

        // Dimension def at the start, before variables
        assert!(
            mdl.contains("region:\n\tnorth, south\n\t~~|"),
            "should contain dimension def"
        );
        let dim_pos = mdl.find("region:").unwrap();
        let var_pos = mdl.find("x=").unwrap();
        assert!(dim_pos < var_pos, "dimensions should come before variables");
    }

    // ---- Phase 5 Task 1: Sketch element serialization (types 10, 11, 12) ----

    #[test]
    fn sketch_aux_element() {
        let aux = view_element::Aux {
            name: "Growth_Rate".to_string(),
            uid: 1,
            x: 100.0,
            y: 200.0,
            label_side: view_element::LabelSide::Bottom,
        };
        let mut buf = String::new();
        write_aux_element(&mut buf, &aux);
        assert_eq!(buf, "10,1,Growth Rate,100,200,40,20,8,3,0,0,-1,0,0,0");
    }

    #[test]
    fn sketch_stock_element() {
        let stock = view_element::Stock {
            name: "Population".to_string(),
            uid: 2,
            x: 300.0,
            y: 150.0,
            label_side: view_element::LabelSide::Top,
        };
        let mut buf = String::new();
        write_stock_element(&mut buf, &stock);
        assert_eq!(buf, "10,2,Population,300,150,40,20,3,3,0,0,0,0,0,0");
    }

    #[test]
    fn sketch_flow_element_produces_valve_and_variable() {
        let flow = view_element::Flow {
            name: "Infection_Rate".to_string(),
            uid: 6,
            x: 295.0,
            y: 191.0,
            label_side: view_element::LabelSide::Bottom,
            points: vec![],
        };
        let mut buf = String::new();
        let valve_uids = HashMap::from([(6, 100)]);
        let mut next_connector_uid = 200;
        write_flow_element(&mut buf, &flow, &valve_uids, &mut next_connector_uid);
        // valve line uses allocated UID, variable line uses flow's UID
        assert!(buf.starts_with("11,100,100,295,191,6,8,34,3,0,0,1,0,0,0\n"));
        assert!(buf.contains("10,6,Infection Rate,295,207,49,8,40,3,0,0,-1,0,0,0"));
    }

    #[test]
    fn sketch_flow_element_emits_pipe_connectors_from_flow_points() {
        let flow = view_element::Flow {
            name: "Infection_Rate".to_string(),
            uid: 6,
            x: 150.0,
            y: 100.0,
            label_side: view_element::LabelSide::Bottom,
            points: vec![
                view_element::FlowPoint {
                    x: 100.0,
                    y: 100.0,
                    attached_to_uid: Some(1),
                },
                view_element::FlowPoint {
                    x: 200.0,
                    y: 100.0,
                    attached_to_uid: Some(2),
                },
            ],
        };
        let mut buf = String::new();
        let valve_uids = HashMap::from([(6, 100)]);
        let mut next_connector_uid = 200;
        write_flow_element(&mut buf, &flow, &valve_uids, &mut next_connector_uid);

        let connector_lines: Vec<&str> =
            buf.lines().filter(|line| line.starts_with("1,")).collect();
        assert_eq!(
            connector_lines.len(),
            2,
            "Expected two type-1 connector lines for flow endpoints: {}",
            buf
        );
        assert!(
            connector_lines.iter().any(|line| line.contains(",100,1,")),
            "Expected connector from valve uid 100 to endpoint uid 1: {}",
            buf
        );
        assert!(
            connector_lines.iter().any(|line| line.contains(",100,2,")),
            "Expected connector from valve uid 100 to endpoint uid 2: {}",
            buf
        );
    }

    #[test]
    fn valve_uids_do_not_collide_with_existing_elements() {
        // stock uid=1, flow uid=2 -> valve must NOT get uid=1
        let elements = vec![
            ViewElement::Stock(view_element::Stock {
                name: "Population".to_string(),
                uid: 1,
                x: 100.0,
                y: 100.0,
                label_side: view_element::LabelSide::Bottom,
            }),
            ViewElement::Flow(view_element::Flow {
                name: "Birth_Rate".to_string(),
                uid: 2,
                x: 200.0,
                y: 100.0,
                label_side: view_element::LabelSide::Bottom,
                points: vec![],
            }),
        ];

        let valve_uids = allocate_valve_uids(&elements);
        // The valve for flow uid=2 must not equal 1 (stock's uid)
        let valve_uid = valve_uids[&2];
        assert_ne!(valve_uid, 1, "Valve UID collides with stock UID");
        assert_ne!(valve_uid, 2, "Valve UID collides with flow UID");
    }

    #[test]
    fn sketch_cloud_element() {
        let cloud = view_element::Cloud {
            uid: 7,
            flow_uid: 6,
            x: 479.0,
            y: 235.0,
        };
        let mut buf = String::new();
        write_cloud_element(&mut buf, &cloud);
        assert_eq!(buf, "12,7,0,479,235,10,8,0,3,0,0,-1,0,0,0");
    }

    #[test]
    fn sketch_alias_element() {
        let alias = view_element::Alias {
            uid: 10,
            alias_of_uid: 1,
            x: 200.0,
            y: 300.0,
            label_side: view_element::LabelSide::Bottom,
        };
        let mut name_map = HashMap::new();
        name_map.insert(1, "Growth_Rate");
        let mut buf = String::new();
        write_alias_element(&mut buf, &alias, &name_map);
        assert!(buf.starts_with("10,10,Growth Rate,200,300,40,20,8,2,0,3,-1,0,0,0,"));
        assert!(buf.contains("128-128-128"));
    }

    // ---- Phase 5 Task 2: Connector serialization (type 1) ----

    #[test]
    fn sketch_link_straight() {
        let link = view_element::Link {
            uid: 3,
            from_uid: 1,
            to_uid: 2,
            shape: LinkShape::Straight,
            polarity: None,
        };
        let mut positions = HashMap::new();
        positions.insert(1, (100, 100));
        positions.insert(2, (200, 200));
        let mut buf = String::new();
        write_link_element(&mut buf, &link, &positions, false);
        // Straight => control point (0,0)
        assert_eq!(buf, "1,3,1,2,0,0,0,0,0,0,0,-1--1--1,,1|(0,0)|");
    }

    #[test]
    fn sketch_link_with_polarity_symbol() {
        let link = view_element::Link {
            uid: 5,
            from_uid: 1,
            to_uid: 2,
            shape: LinkShape::Straight,
            polarity: Some(LinkPolarity::Positive),
        };
        let positions = HashMap::new();
        let mut buf = String::new();
        write_link_element(&mut buf, &link, &positions, false);
        // polarity=43 ('+')
        assert!(buf.contains(",0,0,43,0,0,0,0,"));
    }

    #[test]
    fn sketch_link_with_polarity_letter() {
        let link = view_element::Link {
            uid: 5,
            from_uid: 1,
            to_uid: 2,
            shape: LinkShape::Straight,
            polarity: Some(LinkPolarity::Positive),
        };
        let positions = HashMap::new();
        let mut buf = String::new();
        write_link_element(&mut buf, &link, &positions, true);
        // polarity=83 ('S' for lettered positive)
        assert!(buf.contains(",0,0,83,0,0,0,0,"));
    }

    #[test]
    fn sketch_link_arc_produces_nonzero_control_point() {
        let link = view_element::Link {
            uid: 3,
            from_uid: 1,
            to_uid: 2,
            shape: LinkShape::Arc(45.0),
            polarity: None,
        };
        let mut positions = HashMap::new();
        positions.insert(1, (100, 100));
        positions.insert(2, (200, 100));
        let mut buf = String::new();
        write_link_element(&mut buf, &link, &positions, false);
        // Arc should produce a non-(0,0) control point
        assert!(
            !buf.contains("|(0,0)|"),
            "arc should not produce (0,0) control point"
        );
    }

    #[test]
    fn sketch_link_multipoint_emits_all_points() {
        let points = vec![
            view_element::FlowPoint {
                x: 150.0,
                y: 120.0,
                attached_to_uid: None,
            },
            view_element::FlowPoint {
                x: 170.0,
                y: 140.0,
                attached_to_uid: None,
            },
            view_element::FlowPoint {
                x: 190.0,
                y: 160.0,
                attached_to_uid: None,
            },
        ];
        let link = view_element::Link {
            uid: 4,
            from_uid: 1,
            to_uid: 2,
            shape: LinkShape::MultiPoint(points),
            polarity: None,
        };
        let mut positions = HashMap::new();
        positions.insert(1, (100, 100));
        positions.insert(2, (200, 200));
        let mut buf = String::new();
        write_link_element(&mut buf, &link, &positions, false);
        assert!(
            buf.contains("3|(150,120)|(170,140)|(190,160)|"),
            "multipoint should emit all three points: {buf}"
        );
    }

    // ---- Phase 5 Task 3: Complete sketch section assembly ----

    #[test]
    fn sketch_section_structure() {
        let elements = vec![
            ViewElement::Stock(view_element::Stock {
                name: "Population".to_string(),
                uid: 1,
                x: 100.0,
                y: 100.0,
                label_side: view_element::LabelSide::Top,
            }),
            ViewElement::Aux(view_element::Aux {
                name: "Growth_Rate".to_string(),
                uid: 2,
                x: 200.0,
                y: 200.0,
                label_side: view_element::LabelSide::Bottom,
            }),
            ViewElement::Link(view_element::Link {
                uid: 3,
                from_uid: 2,
                to_uid: 1,
                shape: LinkShape::Straight,
                polarity: None,
            }),
        ];
        let sf = datamodel::StockFlow {
            name: None,
            elements,
            view_box: Default::default(),
            zoom: 1.0,
            use_lettered_polarity: false,
        };
        let views = vec![View::StockFlow(sf)];

        let mut writer = MdlWriter::new();
        writer.write_sketch_section(&views);
        let output = writer.buf;

        // Header
        assert!(
            output.starts_with("V300  Do not put anything below this section"),
            "should start with V300 header"
        );
        // View title
        assert!(output.contains("*View 1\n"), "should have view title");
        // Font line
        assert!(
            output.contains("$192-192-192"),
            "should have font settings line"
        );
        // Elements
        assert!(
            output.contains("10,1,Population,"),
            "should have stock element"
        );
        assert!(
            output.contains("10,2,Growth Rate,"),
            "should have aux element"
        );
        assert!(output.contains("1,3,2,1,"), "should have link element");
        // Terminator
        assert!(
            output.ends_with("///---\\\\\\\n"),
            "should end with sketch terminator"
        );
    }

    #[test]
    fn sketch_section_in_full_project() {
        let var = make_aux("x", "1", None, "");
        let elements = vec![ViewElement::Aux(view_element::Aux {
            name: "x".to_string(),
            uid: 1,
            x: 100.0,
            y: 100.0,
            label_side: view_element::LabelSide::Bottom,
        })];
        let model = datamodel::Model {
            name: "default".to_owned(),
            sim_specs: None,
            variables: vec![var],
            views: vec![View::StockFlow(datamodel::StockFlow {
                name: None,
                elements,
                view_box: Default::default(),
                zoom: 1.0,
                use_lettered_polarity: false,
            })],
            loop_metadata: vec![],
            groups: vec![],
        };
        let project = make_project(vec![model]);

        let result = crate::mdl::project_to_mdl(&project);
        assert!(result.is_ok());
        let mdl = result.unwrap();

        // The sketch section should appear after the equations terminator
        let terminator_pos = mdl
            .find("\\\\\\---/// Sketch information")
            .expect("should have equations terminator");
        let v300_pos = mdl.find("V300").expect("should have V300 header");
        assert!(
            terminator_pos < v300_pos,
            "V300 should come after equations terminator"
        );

        // The sketch terminator should be at the end
        assert!(
            mdl.contains("///---\\\\\\"),
            "should have sketch terminator"
        );
    }

    #[test]
    fn sketch_roundtrip_teacup() {
        // Read teacup.mdl, parse to Project, write sketch section, verify structure
        let mdl_contents = include_str!("../../../../test/test-models/samples/teacup/teacup.mdl");
        let project =
            crate::mdl::parse_mdl(mdl_contents).expect("teacup.mdl should parse successfully");

        let model = &project.models[0];
        assert!(
            !model.views.is_empty(),
            "teacup model should have at least one view"
        );

        // Write the sketch section
        let mut writer = MdlWriter::new();
        writer.write_sketch_section(&model.views);
        let output = writer.buf;

        // Verify structural elements: the teacup model should have stocks, auxes,
        // flows (valve + attached variable), links, and clouds.
        assert!(output.contains("V300"), "output should contain V300 header");
        assert!(
            output.contains("*View 1"),
            "output should contain view title"
        );
        assert!(
            output.contains("///---\\\\\\"),
            "output should end with sketch terminator"
        );

        // The teacup model elements (after roundtrip through datamodel):
        // Stock: Teacup_Temperature -> type 10 with shape=3
        // Aux: Heat_Loss_to_Room flow -> type 11 valve + type 10 attached
        // Aux: Room_Temperature, Characteristic_Time -> type 10
        // Links -> type 1
        // Clouds -> type 12

        // Count element types in output
        let lines: Vec<&str> = output.lines().collect();
        let type10_count = lines.iter().filter(|l| l.starts_with("10,")).count();
        let type11_count = lines.iter().filter(|l| l.starts_with("11,")).count();
        let type12_count = lines.iter().filter(|l| l.starts_with("12,")).count();
        let type1_count = lines.iter().filter(|l| l.starts_with("1,")).count();

        // Teacup has: 1 stock (Teacup_Temperature), 3 auxes (Heat_Loss_to_Room,
        // Room_Temperature, Characteristic_Time), 1 flow (Heat_Loss_to_Room)
        // which produces valve+variable, plus 1 cloud.
        // The exact numbers depend on the MDL->datamodel conversion, but
        // we should have a reasonable set of elements.
        assert!(
            type10_count >= 2,
            "should have at least 2 type-10 elements (variables/stocks), got {type10_count}"
        );
        assert!(
            type11_count >= 1,
            "should have at least 1 type-11 element (valve), got {type11_count}"
        );
        assert!(
            type12_count >= 1,
            "should have at least 1 type-12 element (cloud/comment), got {type12_count}"
        );
        assert!(
            type1_count >= 1,
            "should have at least 1 type-1 element (connector), got {type1_count}"
        );
        // Verify no empty lines were introduced between elements
        let element_lines: Vec<&&str> = lines
            .iter()
            .filter(|l| {
                l.starts_with("10,")
                    || l.starts_with("11,")
                    || l.starts_with("12,")
                    || l.starts_with("1,")
            })
            .collect();
        assert!(
            !element_lines.is_empty(),
            "should have sketch elements in output"
        );

        // Verify the output can be re-parsed as a valid sketch section
        let reparsed = crate::mdl::view::parse_views(&output);
        assert!(
            reparsed.is_ok(),
            "re-serialized sketch should parse: {:?}",
            reparsed.err()
        );
        let views = reparsed.unwrap();
        assert!(
            !views.is_empty(),
            "re-parsed sketch should have at least one view"
        );

        // Verify all expected element types are present after re-parse
        let view = &views[0];
        let has_variable = view
            .iter()
            .any(|e| matches!(e, crate::mdl::view::VensimElement::Variable(_)));
        let has_connector = view
            .iter()
            .any(|e| matches!(e, crate::mdl::view::VensimElement::Connector(_)));
        assert!(has_variable, "re-parsed view should have variables");
        assert!(has_connector, "re-parsed view should have connectors");
    }

    #[test]
    fn sketch_roundtrip_preserves_view_title() {
        let mdl_contents = r#"x = 5
~ ~|
\\\---/// Sketch information
V300  Do not put anything below this section - it will be ignored
*Overview
$192-192-192,0,Times New Roman|12||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|96,96,100,0
10,1,x,100,100,40,20,8,3,0,0,-1,0,0,0
///---\\\
"#;

        let project =
            crate::mdl::parse_mdl(mdl_contents).expect("source MDL should parse successfully");
        let mdl = crate::mdl::project_to_mdl(&project).expect("roundtrip MDL write should work");

        assert!(
            mdl.contains("*Overview\n"),
            "Roundtrip should preserve original view title: {}",
            mdl
        );
    }

    #[test]
    fn sketch_roundtrip_sanitizes_multiline_view_title() {
        let var = make_aux("x", "5", Some("Units"), "A constant");
        let model = datamodel::Model {
            name: "default".to_owned(),
            sim_specs: None,
            variables: vec![var],
            views: vec![View::StockFlow(datamodel::StockFlow {
                name: Some("Overview\r\nMain".to_owned()),
                elements: vec![ViewElement::Aux(view_element::Aux {
                    name: "x".to_owned(),
                    uid: 1,
                    x: 100.0,
                    y: 100.0,
                    label_side: view_element::LabelSide::Bottom,
                })],
                view_box: Default::default(),
                zoom: 1.0,
                use_lettered_polarity: false,
            })],
            loop_metadata: vec![],
            groups: vec![],
        };
        let project = make_project(vec![model]);

        let mdl = crate::mdl::project_to_mdl(&project).expect("MDL write should succeed");
        assert!(
            mdl.contains("*Overview Main\n"),
            "view title should be serialized as a single line: {mdl}",
        );

        let reparsed = crate::mdl::parse_mdl(&mdl).expect("written MDL should parse");
        let View::StockFlow(sf) = &reparsed.models[0].views[0];
        assert_eq!(
            sf.name.as_deref(),
            Some("Overview Main"),
            "sanitized title should roundtrip through MDL",
        );
    }

    #[test]
    fn sketch_roundtrip_preserves_flow_endpoints_with_nonadjacent_valve_uid() {
        let stock_a = Variable::Stock(Stock {
            ident: "stock_a".to_owned(),
            equation: Equation::Scalar("100".to_owned()),
            documentation: String::new(),
            units: None,
            inflows: vec![],
            outflows: vec!["flow_ab".to_owned()],
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        let stock_b = Variable::Stock(Stock {
            ident: "stock_b".to_owned(),
            equation: Equation::Scalar("0".to_owned()),
            documentation: String::new(),
            units: None,
            inflows: vec!["flow_ab".to_owned()],
            outflows: vec![],
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        let flow = Variable::Flow(Flow {
            ident: "flow_ab".to_owned(),
            equation: Equation::Scalar("10".to_owned()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });

        let model = datamodel::Model {
            name: "default".to_owned(),
            sim_specs: None,
            variables: vec![stock_a, stock_b, flow],
            views: vec![View::StockFlow(datamodel::StockFlow {
                name: Some("View 1".to_owned()),
                elements: vec![
                    ViewElement::Stock(view_element::Stock {
                        name: "Stock_A".to_owned(),
                        uid: 1,
                        x: 100.0,
                        y: 100.0,
                        label_side: view_element::LabelSide::Bottom,
                    }),
                    ViewElement::Stock(view_element::Stock {
                        name: "Stock_B".to_owned(),
                        uid: 2,
                        x: 300.0,
                        y: 100.0,
                        label_side: view_element::LabelSide::Bottom,
                    }),
                    ViewElement::Flow(view_element::Flow {
                        name: "Flow_AB".to_owned(),
                        uid: 6,
                        x: 200.0,
                        y: 100.0,
                        label_side: view_element::LabelSide::Bottom,
                        points: vec![
                            view_element::FlowPoint {
                                x: 122.5,
                                y: 100.0,
                                attached_to_uid: Some(1),
                            },
                            view_element::FlowPoint {
                                x: 277.5,
                                y: 100.0,
                                attached_to_uid: Some(2),
                            },
                        ],
                    }),
                ],
                view_box: Default::default(),
                zoom: 1.0,
                use_lettered_polarity: false,
            })],
            loop_metadata: vec![],
            groups: vec![],
        };
        let project = make_project(vec![model]);

        let mdl = crate::mdl::project_to_mdl(&project).expect("MDL write should succeed");
        let reparsed = crate::mdl::parse_mdl(&mdl).expect("written MDL should parse");
        let View::StockFlow(sf) = &reparsed.models[0].views[0];

        let stock_uid_by_name: HashMap<&str, i32> = sf
            .elements
            .iter()
            .filter_map(|elem| {
                if let ViewElement::Stock(stock) = elem {
                    Some((stock.name.as_str(), stock.uid))
                } else {
                    None
                }
            })
            .collect();

        let flow = sf
            .elements
            .iter()
            .find_map(|elem| {
                if let ViewElement::Flow(flow) = elem {
                    Some(flow)
                } else {
                    None
                }
            })
            .expect("expected flow element after roundtrip");

        assert_eq!(
            flow.points.first().and_then(|pt| pt.attached_to_uid),
            stock_uid_by_name.get("Stock_A").copied(),
            "flow source attachment should roundtrip to Stock_A",
        );
        assert_eq!(
            flow.points.last().and_then(|pt| pt.attached_to_uid),
            stock_uid_by_name.get("Stock_B").copied(),
            "flow sink attachment should roundtrip to Stock_B",
        );
    }

    #[test]
    fn compute_control_point_straight_midpoint() {
        // For a nearly-straight arc angle, the control point should be near the midpoint
        let from = (100, 100);
        let to = (200, 100);
        // Canvas angle of 0 degrees = straight line along x-axis
        let (cx, cy) = compute_control_point(from, to, 0.0);
        // For a straight line, the midpoint should be returned
        assert_eq!(cx, 150);
        assert_eq!(cy, 100);
    }

    #[test]
    fn compute_control_point_arc_off_center() {
        // A 45-degree arc should produce a control point off the midpoint
        let from = (100, 100);
        let to = (200, 100);
        let (_cx, cy) = compute_control_point(from, to, 45.0);
        // The control point should be above or below the line, not on it
        assert_ne!(cy, 100, "arc control point should be off the straight line");
    }

    // ---- Phase 6 Task 1: Settings section ----

    #[test]
    fn settings_section_starts_with_marker() {
        let project = make_project(vec![make_model(vec![])]);
        let mut writer = MdlWriter::new();
        writer.write_settings_section(&project);
        let output = writer.buf;
        assert!(
            output.starts_with(":L<%^E!@\n"),
            "settings section should start with marker (separator is in sketch section), got: {:?}",
            &output[..output.len().min(40)]
        );
    }

    #[test]
    fn settings_section_contains_type_15_euler() {
        let project = make_project(vec![make_model(vec![])]);
        let mut writer = MdlWriter::new();
        writer.write_settings_section(&project);
        let output = writer.buf;
        assert!(
            output.contains("15:0,0,0,0,0,0\n"),
            "Euler method should emit method code 0, got: {:?}",
            output
        );
    }

    #[test]
    fn settings_section_contains_type_15_rk4() {
        let mut project = make_project(vec![make_model(vec![])]);
        project.sim_specs.sim_method = SimMethod::RungeKutta4;
        let mut writer = MdlWriter::new();
        writer.write_settings_section(&project);
        let output = writer.buf;
        assert!(
            output.contains("15:0,0,0,1,0,0\n"),
            "RK4 method should emit method code 1, got: {:?}",
            output
        );
    }

    #[test]
    fn settings_section_contains_type_15_rk2() {
        let mut project = make_project(vec![make_model(vec![])]);
        project.sim_specs.sim_method = SimMethod::RungeKutta2;
        let mut writer = MdlWriter::new();
        writer.write_settings_section(&project);
        let output = writer.buf;
        assert!(
            output.contains("15:0,0,0,3,0,0\n"),
            "RK2 method should emit method code 3, got: {:?}",
            output
        );
    }

    #[test]
    fn settings_section_contains_type_22_units() {
        let mut project = make_project(vec![make_model(vec![])]);
        project.units = vec![
            Unit {
                name: "Dollar".to_owned(),
                equation: Some("$".to_owned()),
                disabled: false,
                aliases: vec!["Dollars".to_owned(), "$s".to_owned()],
            },
            Unit {
                name: "Hour".to_owned(),
                equation: None,
                disabled: false,
                aliases: vec!["Hours".to_owned()],
            },
        ];
        let mut writer = MdlWriter::new();
        writer.write_settings_section(&project);
        let output = writer.buf;
        assert!(
            output.contains("22:$,Dollar,Dollars,$s\n"),
            "should contain Dollar unit equivalence, got: {:?}",
            output
        );
        assert!(
            output.contains("22:Hour,Hours\n"),
            "should contain Hour unit equivalence, got: {:?}",
            output
        );
    }

    #[test]
    fn settings_section_skips_disabled_units() {
        let mut project = make_project(vec![make_model(vec![])]);
        project.units = vec![Unit {
            name: "Disabled".to_owned(),
            equation: None,
            disabled: true,
            aliases: vec![],
        }];
        let mut writer = MdlWriter::new();
        writer.write_settings_section(&project);
        let output = writer.buf;
        assert!(
            !output.contains("22:Disabled"),
            "disabled units should not appear in output"
        );
    }

    #[test]
    fn settings_section_contains_common_defaults() {
        let project = make_project(vec![make_model(vec![])]);
        let mut writer = MdlWriter::new();
        writer.write_settings_section(&project);
        let output = writer.buf;
        // Type 4 (Time), Type 19 (display), Type 24/25/26 (time bounds)
        assert!(output.contains("\n4:Time\n"), "should have Type 4 (Time)");
        assert!(
            output.contains("\n19:"),
            "should have Type 19 (display settings)"
        );
        assert!(
            output.contains("\n24:"),
            "should have Type 24 (initial time)"
        );
        assert!(output.contains("\n25:"), "should have Type 25 (final time)");
        assert!(output.contains("\n26:"), "should have Type 26 (time step)");
    }

    #[test]
    fn settings_roundtrip_integration_method() {
        // Write settings, then parse them back and check integration method
        for method in [
            SimMethod::Euler,
            SimMethod::RungeKutta4,
            SimMethod::RungeKutta2,
        ] {
            let mut project = make_project(vec![make_model(vec![])]);
            project.sim_specs.sim_method = method;
            let mut writer = MdlWriter::new();
            writer.write_settings_section(&project);
            // Prepend the separator that write_sketch_section normally emits
            let output = format!("///---\\\\\\\n{}", writer.buf);

            let parser = crate::mdl::settings::PostEquationParser::new(&output);
            let settings = parser.parse_settings();
            assert_eq!(
                settings.integration_method, method,
                "integration method should roundtrip for {:?}",
                method
            );
        }
    }

    #[test]
    fn settings_roundtrip_unit_equivalences() {
        let mut project = make_project(vec![make_model(vec![])]);
        project.units = vec![
            Unit {
                name: "Dollar".to_owned(),
                equation: Some("$".to_owned()),
                disabled: false,
                aliases: vec!["Dollars".to_owned()],
            },
            Unit {
                name: "Hour".to_owned(),
                equation: None,
                disabled: false,
                aliases: vec!["Hours".to_owned(), "Hr".to_owned()],
            },
        ];
        let mut writer = MdlWriter::new();
        writer.write_settings_section(&project);
        // Prepend the separator that write_sketch_section normally emits
        let output = format!("///---\\\\\\\n{}", writer.buf);

        let parser = crate::mdl::settings::PostEquationParser::new(&output);
        let settings = parser.parse_settings();
        assert_eq!(settings.unit_equivs.len(), 2);
        assert_eq!(settings.unit_equivs[0].name, "Dollar");
        assert_eq!(settings.unit_equivs[0].equation, Some("$".to_string()));
        assert_eq!(settings.unit_equivs[0].aliases, vec!["Dollars"]);
        assert_eq!(settings.unit_equivs[1].name, "Hour");
        assert_eq!(settings.unit_equivs[1].equation, None);
        assert_eq!(settings.unit_equivs[1].aliases, vec!["Hours", "Hr"]);
    }

    // ---- Phase 6 Task 2: Full file assembly ----

    #[test]
    fn full_assembly_has_all_three_sections() {
        let var = make_aux("x", "5", Some("Units"), "A constant");
        let elements = vec![ViewElement::Aux(view_element::Aux {
            name: "x".to_string(),
            uid: 1,
            x: 100.0,
            y: 100.0,
            label_side: view_element::LabelSide::Bottom,
        })];
        let model = datamodel::Model {
            name: "default".to_owned(),
            sim_specs: None,
            variables: vec![var],
            views: vec![View::StockFlow(datamodel::StockFlow {
                name: None,
                elements,
                view_box: Default::default(),
                zoom: 1.0,
                use_lettered_polarity: false,
            })],
            loop_metadata: vec![],
            groups: vec![],
        };
        let project = make_project(vec![model]);

        let result = crate::mdl::project_to_mdl(&project);
        assert!(
            result.is_ok(),
            "project_to_mdl should succeed: {:?}",
            result
        );
        let mdl = result.unwrap();

        // Section 1: Equations -- contains variable entry
        assert!(mdl.contains("x="), "should contain equation for x");
        // Equations terminator
        assert!(
            mdl.contains("\\\\\\---/// Sketch information"),
            "should have equations terminator"
        );

        // Section 2: Sketch -- V300 header and elements
        assert!(mdl.contains("V300"), "should have V300 sketch header");
        assert!(mdl.contains("*View 1"), "should have view title");

        // Section 3: Settings -- marker and type codes
        assert!(mdl.contains(":L<%^E!@"), "should have settings marker");
        assert!(mdl.contains("15:"), "should have Type 15 line");

        // Sections should be in order: equations, sketch, settings
        let eq_term = mdl.find("\\\\\\---/// Sketch").unwrap();
        let v300 = mdl.find("V300").unwrap();
        let sketch_term = mdl.find("///---\\\\\\").unwrap();
        let settings_marker = mdl.find(":L<%^E!@").unwrap();
        assert!(eq_term < v300, "equations should come before sketch");
        assert!(
            v300 < sketch_term,
            "V300 should come before sketch terminator"
        );
        assert!(
            sketch_term < settings_marker,
            "sketch terminator should come before settings marker"
        );
    }

    // ---- Phase 6 Task 3: compat wrapper ----

    #[test]
    fn compat_to_mdl_matches_project_to_mdl() {
        let var = make_aux("x", "5", Some("Units"), "A constant");
        let project = make_project(vec![make_model(vec![var])]);

        let direct = crate::mdl::project_to_mdl(&project).unwrap();
        let compat = crate::compat::to_mdl(&project).unwrap();
        assert_eq!(
            direct, compat,
            "compat::to_mdl should produce same result as mdl::project_to_mdl"
        );
    }

    #[test]
    fn write_arrayed_with_default_equation_omits_dimension_level_default() {
        // When default_equation is set (from EXCEPT syntax), the writer must
        // NOT emit name[Dim...]=default because that would apply the default
        // equation to excepted elements that should default to 0.
        let var = datamodel::Variable::Aux(datamodel::Aux {
            ident: "cost".to_string(),
            equation: datamodel::Equation::Arrayed(
                vec!["region".to_string()],
                vec![
                    ("north".to_string(), "base+1".to_string(), None, None),
                    ("south".to_string(), "base+2".to_string(), None, None),
                ],
                Some("base".to_string()),
            ),
            documentation: String::new(),
            units: Some("dollars".to_string()),
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        });

        let mut buf = String::new();
        write_variable_entry(&mut buf, &var);

        // Must NOT contain dimension-level default (would apply to excepted elements)
        assert!(
            !buf.contains("cost[region]="),
            "should NOT contain dimension-level default equation, got: {buf}"
        );
        // Individual element equations should be present
        assert!(
            buf.contains("cost[north]="),
            "should contain north element equation, got: {buf}"
        );
        assert!(
            buf.contains("cost[south]="),
            "should contain south element equation, got: {buf}"
        );
    }

    #[test]
    fn write_dimension_with_element_level_mapping() {
        let dim = datamodel::Dimension {
            name: "dim_a".to_string(),
            elements: datamodel::DimensionElements::Named(vec!["a1".to_string(), "a2".to_string()]),
            mappings: vec![datamodel::DimensionMapping {
                target: "dim_b".to_string(),
                element_map: vec![
                    ("a1".to_string(), "b2".to_string()),
                    ("a2".to_string(), "b1".to_string()),
                ],
            }],
        };

        let mut buf = String::new();
        write_dimension_def(&mut buf, &dim);

        assert!(
            buf.contains("-> (dim b: b2, b1)"),
            "element-level mapping must use parenthesized syntax, got: {buf}"
        );
    }

    #[test]
    fn write_dimension_with_multi_target_positional_mapping() {
        let dim = datamodel::Dimension {
            name: "dim_a".to_string(),
            elements: datamodel::DimensionElements::Named(vec!["a1".to_string(), "a2".to_string()]),
            mappings: vec![
                datamodel::DimensionMapping {
                    target: "dim_b".to_string(),
                    element_map: vec![],
                },
                datamodel::DimensionMapping {
                    target: "dim_c".to_string(),
                    element_map: vec![],
                },
            ],
        };

        let mut buf = String::new();
        write_dimension_def(&mut buf, &dim);

        assert!(
            buf.contains("dim b") && buf.contains("dim c"),
            "both positional mapping targets should be emitted, got: {buf}"
        );
    }

    #[test]
    fn write_dimension_element_mapping_sorted_by_source_position() {
        // element_map entries out of source order should still emit
        // targets in the dimension's element order for correct positional
        // correspondence on re-import.
        let dim = datamodel::Dimension {
            name: "dim_a".to_string(),
            elements: datamodel::DimensionElements::Named(vec![
                "a1".to_string(),
                "a2".to_string(),
                "a3".to_string(),
            ]),
            mappings: vec![datamodel::DimensionMapping {
                target: "dim_b".to_string(),
                element_map: vec![
                    ("a3".to_string(), "b3".to_string()),
                    ("a1".to_string(), "b1".to_string()),
                    ("a2".to_string(), "b2".to_string()),
                ],
            }],
        };

        let mut buf = String::new();
        write_dimension_def(&mut buf, &dim);

        assert!(
            buf.contains("-> (dim b: b1, b2, b3)"),
            "targets should be in source element order (a1->b1, a2->b2, a3->b3), got: {buf}"
        );
    }

    #[test]
    fn write_dimension_element_mapping_case_insensitive_lookup() {
        // element_map uses canonical (lowercase) keys, but dim.elements
        // may preserve original casing -- the sort must still work.
        let dim = datamodel::Dimension {
            name: "Region".to_string(),
            elements: datamodel::DimensionElements::Named(vec![
                "North".to_string(),
                "South".to_string(),
                "East".to_string(),
            ]),
            mappings: vec![datamodel::DimensionMapping {
                target: "zone".to_string(),
                element_map: vec![
                    ("east".to_string(), "z3".to_string()),
                    ("north".to_string(), "z1".to_string()),
                    ("south".to_string(), "z2".to_string()),
                ],
            }],
        };

        let mut buf = String::new();
        write_dimension_def(&mut buf, &dim);

        assert!(
            buf.contains("-> (zone: z1, z2, z3)"),
            "targets should be sorted by source element order despite case mismatch, got: {buf}"
        );
    }

    #[test]
    fn write_dimension_one_to_many_falls_back_to_positional() {
        // When a source element maps to multiple targets (from subdimension
        // expansion), the element-level notation can't round-trip correctly.
        // The writer should fall back to a positional dimension-name mapping.
        let dim = datamodel::Dimension {
            name: "dim_b".to_string(),
            elements: datamodel::DimensionElements::Named(vec!["b1".to_string(), "b2".to_string()]),
            mappings: vec![datamodel::DimensionMapping {
                target: "dim_a".to_string(),
                element_map: vec![
                    ("b1".to_string(), "a1".to_string()),
                    ("b1".to_string(), "a2".to_string()),
                    ("b2".to_string(), "a3".to_string()),
                ],
            }],
        };

        let mut buf = String::new();
        write_dimension_def(&mut buf, &dim);

        assert!(
            buf.contains("-> dim a") && !buf.contains("(dim a:"),
            "one-to-many mapping should fall back to positional notation, got: {buf}"
        );
    }

    #[test]
    fn write_arrayed_with_default_equation_writes_explicit_elements() {
        let mut buf = String::new();
        write_arrayed_entries(
            &mut buf,
            "g",
            &["DimA".to_string()],
            &[
                ("A1".to_string(), "10".to_string(), None, None),
                ("A2".to_string(), "7".to_string(), None, None),
                ("A3".to_string(), "7".to_string(), None, None),
            ],
            &Some("7".to_string()),
            &None,
            "",
        );
        assert!(
            !buf.contains("g[DimA]"),
            "dimension-level default must not be emitted, got: {buf}"
        );
        assert!(
            !buf.contains(":EXCEPT:"),
            "EXCEPT syntax should not be emitted"
        );
        assert!(
            buf.contains("g[A1]"),
            "A1 entry should be written explicitly, got: {buf}"
        );
        assert!(
            buf.contains("g[A2]") && buf.contains("g[A3]"),
            "all explicit array elements should be written, got: {buf}"
        );
    }

    #[test]
    fn write_arrayed_no_default_writes_all_elements() {
        let mut buf = String::new();
        write_arrayed_entries(
            &mut buf,
            "h",
            &["DimA".to_string()],
            &[
                ("A1".to_string(), "8".to_string(), None, None),
                ("A2".to_string(), "0".to_string(), None, None),
            ],
            &None,
            &None,
            "",
        );
        assert!(
            !buf.contains(":EXCEPT:"),
            "should not emit EXCEPT when no default_equation, got: {buf}"
        );
        assert!(buf.contains("h[A1]"), "should write A1 element, got: {buf}");
        assert!(buf.contains("h[A2]"), "should write A2 element, got: {buf}");
    }

    #[test]
    fn write_arrayed_except_no_exceptions_all_default() {
        let mut buf = String::new();
        write_arrayed_entries(
            &mut buf,
            "k",
            &["DimA".to_string()],
            &[
                ("A1".to_string(), "5".to_string(), None, None),
                ("A2".to_string(), "5".to_string(), None, None),
            ],
            &Some("5".to_string()),
            &None,
            "",
        );
        assert!(
            !buf.contains("k[DimA]"),
            "dimension-level default must not be emitted, got: {buf}"
        );
        assert!(buf.contains("k[A1]"), "should write A1 element, got: {buf}");
        assert!(buf.contains("k[A2]"), "should write A2 element, got: {buf}");
        assert!(
            !buf.contains(":EXCEPT:"),
            "EXCEPT syntax should not be emitted, got: {buf}"
        );
    }

    #[test]
    fn write_arrayed_except_with_omitted_elements_avoids_dimension_default() {
        let mut buf = String::new();
        write_arrayed_entries(
            &mut buf,
            "h",
            &["DimA".to_string()],
            &[("A1".to_string(), "8".to_string(), None, None)],
            &Some("8".to_string()),
            &None,
            "",
        );

        assert!(
            !buf.contains("h[DimA]"),
            "dimension-level default would apply to omitted EXCEPT elements, got: {buf}"
        );
        assert!(
            buf.contains("h[A1]"),
            "explicitly present elements must still be emitted, got: {buf}"
        );
    }
}
