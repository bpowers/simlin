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

use super::builtins::to_lower_space;
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
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // Literal newlines must become the two-character escape `\n`.
            '\n' => escaped.push_str("\\n"),
            '\\' => {
                if chars.peek() == Some(&'n') {
                    // XMILE name attributes may contain the literal two-char
                    // sequence `\n` (backslash + 'n') as a display newline.
                    // Preserve it as-is rather than double-escaping to `\\n`.
                    escaped.push_str("\\n");
                    chars.next();
                } else {
                    escaped.push_str("\\\\");
                }
            }
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

/// Build a mapping from canonical variable ident to display name (with
/// original casing, spaces instead of underscores) by walking view elements.
///
/// The first occurrence of a name wins, so if a variable appears in multiple
/// views the first view's casing is used.
fn build_display_name_map(views: &[View]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for view in views {
        let View::StockFlow(sf) = view;
        for element in &sf.elements {
            let name = match element {
                ViewElement::Aux(a) => &a.name,
                ViewElement::Stock(s) => &s.name,
                ViewElement::Flow(f) => &f.name,
                _ => continue,
            };
            let normalized_name = name.replace("\\n", " ").replace('\n', " ");
            let canonical = crate::common::canonicalize(&normalized_name).into_owned();
            let display = underbar_to_space(&normalized_name);
            map.entry(canonical).or_insert(display);
        }
    }
    map
}

/// Look up the display name for a canonical ident, falling back to
/// `format_mdl_ident` if no view element provides original casing.
fn display_name_for_ident(ident: &str, display_names: &HashMap<String, String>) -> String {
    match display_names.get(ident) {
        Some(name) => {
            let name = name.replace("\\n", " ").replace('\n', " ");
            if needs_mdl_quoting(&name) {
                format!("\"{}\"", escape_mdl_quoted_ident(&name))
            } else {
                name
            }
        }
        None => format_mdl_ident(ident),
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

/// Match ALLOCATE BY PRIORITY in both forms:
/// - Native: `allocate_by_priority(request, priority, size, width, supply)`
/// - Legacy: `allocate(supply, last_subscript_ident, demand_with_star, priority, width)`
fn recognize_allocate(expr: &Expr0, walk: &mut impl FnMut(&Expr0) -> String) -> Option<String> {
    if let Expr0::App(UntypedBuiltinFn(f, args), _) = expr {
        // Native form: allocate_by_priority(request, priority, size, width, supply)
        // Args are already in MDL order.
        if f == "allocate_by_priority" && args.len() == 5 {
            let request = walk(&args[0]);
            let priority = walk(&args[1]);
            let size = walk(&args[2]);
            let width = walk(&args[3]);
            let supply = walk(&args[4]);
            return Some(format!(
                "ALLOCATE BY PRIORITY({request}, {priority}, {size}, {width}, {supply})"
            ));
        }

        // Legacy form: allocate(supply, last_subscript, demand_with_star, priority, width)
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
                // Vensim lookup calls use `table_name ( input )` syntax
                // rather than `LOOKUP(table_name, input)`.
                if func == "lookup"
                    && args.len() == 2
                    && let Expr0::Var(table_ident, _) = &args[0]
                {
                    let table_name = format_mdl_ident(table_ident.as_str());
                    let input = self.walk(&args[1]);
                    return format!("{table_name} ( {input} )");
                }
                // safediv with 3+ args is XIDZ (3-arg form), not ZIDZ (2-arg)
                let mdl_name = if func == "safediv" && args.len() >= 3 {
                    "XIDZ".to_owned()
                } else {
                    xmile_to_mdl_function_name(func)
                };
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

/// Write the inner body of a lookup table into `buf` (no outer parens).
///
/// Format: `[(xmin,ymin)-(xmax,ymax)],(x1,y1),(x2,y2),...`
fn write_lookup_body(buf: &mut String, gf: &GraphicalFunction) {
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
        "[({},{})-({},{})]",
        format_f64(gf.x_scale.min),
        format_f64(gf.y_scale.min),
        format_f64(gf.x_scale.max),
        format_f64(gf.y_scale.max),
    )
    .unwrap();

    for (x, y) in xs.iter().zip(gf.y_points.iter()) {
        write!(buf, ",({},{})", format_f64(*x), format_f64(*y)).unwrap();
    }
}

/// Write a graphical-function (lookup table) wrapped in parens.
///
/// Format: `([(xmin,ymin)-(xmax,ymax)],(x1,y1),(x2,y2),...)`
fn write_lookup(buf: &mut String, gf: &GraphicalFunction) {
    buf.push('(');
    write_lookup_body(buf, gf);
    buf.push(')');
}

/// Returns true when the equation text is a placeholder sentinel rather
/// than a real input expression (standalone lookup definition).
///
/// The MDL parser produces [`LOOKUP_SENTINEL`](super::LOOKUP_SENTINEL)
/// when a variable is defined as a pure lookup (no input expression) --
/// see `MdlEquation::Lookup` in `convert/variables.rs`.  Vensim's native
/// representation is `name(body)` rather than `name = WITH LOOKUP(input,
/// body)`.  An empty string covers XMILE variables that have a graphical
/// function but no equation.
fn is_lookup_only_equation(eqn: &str) -> bool {
    let trimmed = eqn.trim();
    trimmed.is_empty() || trimmed == super::LOOKUP_SENTINEL
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
pub fn write_variable_entry(
    buf: &mut String,
    var: &datamodel::Variable,
    display_names: &HashMap<String, String>,
) {
    match var {
        datamodel::Variable::Stock(s) => {
            write_stock_variable(buf, s, display_names);
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
    let name = display_name_for_ident(ident, display_names);

    match equation {
        Equation::Scalar(eqn) => {
            let effective_eqn = data_source_eqn
                .clone()
                .unwrap_or_else(|| wrap_active_initial(eqn, compat));
            write_single_entry(buf, &name, &effective_eqn, &[], units, doc, effective_gf);
        }
        Equation::ApplyToAll(dims, eqn) => {
            let dim_names: Vec<&str> = dims.iter().map(|d| d.as_str()).collect();
            let effective_eqn = data_source_eqn
                .clone()
                .unwrap_or_else(|| wrap_active_initial(eqn, compat));
            write_single_entry(
                buf,
                &name,
                &effective_eqn,
                &dim_names,
                units,
                doc,
                effective_gf,
            );
        }
        Equation::Arrayed(dims, elements, default_eq, _) => {
            write_arrayed_entries(buf, &name, dims, elements, default_eq, units, doc);
        }
    }
}

fn compat_get_direct_equation(compat: &datamodel::Compat) -> Option<String> {
    let ds = compat.data_source.as_ref()?;
    // Vensim's GET DIRECT argument parser uses single quotes as toggle
    // delimiters with no escape mechanism, so we pass arguments through
    // unmodified rather than producing `\'` which would be unparsable.
    let quote = |s: &str| s.to_string();
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
fn write_stock_variable(
    buf: &mut String,
    stock: &datamodel::Stock,
    display_names: &HashMap<String, String>,
) {
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

    let name = display_name_for_ident(&stock.ident, display_names);

    match &stock.equation {
        Equation::Scalar(eqn) => write_stock_entry(
            buf,
            &name,
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
                &name,
                &net_flow,
                &equation_to_mdl(eqn),
                &dim_names,
                &stock.units,
                &stock.documentation,
            );
        }
        Equation::Arrayed(dims, elements, default_eq, _) => {
            write_arrayed_stock_entries(
                buf,
                &name,
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

/// `name` is the pre-formatted display name (with original casing).
fn write_stock_entry(
    buf: &mut String,
    name: &str,
    net_flow: &str,
    initial: &str,
    dims: &[&str],
    units: &Option<String>,
    doc: &str,
) {
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
    name: &str,
    net_flow: &str,
    _dims: &[String],
    elements: &[(String, String, Option<String>, Option<GraphicalFunction>)],
    _default_equation: &Option<String>,
    units: &Option<String>,
    doc: &str,
) {
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

/// Split an MDL equation string into tokens suitable for line wrapping.
///
/// Tokens preserve the original text exactly -- concatenating them yields
/// the input.  The split points are chosen so that line breaks can be
/// inserted *between* tokens at natural boundaries: after commas (with
/// their trailing space), before binary operators, or after open parens.
fn tokenize_for_wrapping(eqn: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = eqn.chars().peekable();

    while let Some(&c) = chars.peek() {
        match c {
            ',' => {
                current.push(chars.next().unwrap());
                // Absorb trailing space after comma so it stays with the comma token
                if chars.peek() == Some(&' ') {
                    current.push(chars.next().unwrap());
                }
                tokens.push(std::mem::take(&mut current));
            }
            '(' | ')' | '[' | ']' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                current.push(chars.next().unwrap());
                tokens.push(std::mem::take(&mut current));
            }
            '+' | '-' | '*' | '/' | '^' => {
                // Emit the accumulated text before the operator so a
                // line break can be inserted before the operator.
                // But first check if this minus/plus is at the very start
                // or follows an operator/open-paren (i.e. is unary).
                let is_unary = current.is_empty()
                    && tokens.last().is_none_or(|t| {
                        let trimmed = t.trim();
                        trimmed.is_empty()
                            || trimmed.ends_with('(')
                            || trimmed.ends_with(',')
                            || trimmed == "+"
                            || trimmed == "-"
                            || trimmed == "*"
                            || trimmed == "/"
                            || trimmed == "^"
                    });
                if is_unary {
                    current.push(chars.next().unwrap());
                } else {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                    // Emit the operator as its own token so line breaks can be inserted before it.
                    current.push(chars.next().unwrap());
                    tokens.push(std::mem::take(&mut current));
                }
            }
            '\'' => {
                // Quoted literal -- consume the whole thing as one piece
                current.push(chars.next().unwrap());
                while let Some(&ch) = chars.peek() {
                    current.push(chars.next().unwrap());
                    if ch == '\'' {
                        break;
                    }
                }
            }
            '"' => {
                // Quoted identifier -- consume the whole thing
                current.push(chars.next().unwrap());
                while let Some(&ch) = chars.peek() {
                    current.push(chars.next().unwrap());
                    if ch == '"' {
                        break;
                    }
                }
            }
            _ => {
                current.push(chars.next().unwrap());
            }
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

/// Wrap a long equation with backslash-newline continuations in Vensim style.
///
/// Short equations (fitting within `max_line_len` characters) pass through
/// unchanged.  Longer ones are split at token boundaries with `\\\n\t\t`
/// continuation sequences (backslash, newline, two tabs for continuation
/// indent under the single-tab equation indent).
fn wrap_equation_with_continuations(eqn: &str, max_line_len: usize) -> String {
    if eqn.len() <= max_line_len {
        return eqn.to_string();
    }

    let tokens = tokenize_for_wrapping(eqn);
    let mut result = String::new();
    let mut current_line_len: usize = 0;

    for token in &tokens {
        // If adding this token would exceed the limit and we already have
        // content on the current line, break before it.
        if current_line_len + token.len() > max_line_len && current_line_len > 0 {
            // Trim trailing whitespace from the current line before the break
            let trimmed_end = result.trim_end_matches(' ').len();
            result.truncate(trimmed_end);
            result.push_str("\\\n\t\t");
            current_line_len = 0;
        }
        result.push_str(token);
        current_line_len += token.len();
    }

    result
}

/// Write one MDL entry (scalar or apply-to-all).
///
/// `name` is the pre-formatted display name (with original casing from
/// view elements, or `format_mdl_ident` fallback).
fn write_single_entry(
    buf: &mut String,
    name: &str,
    eqn: &str,
    dims: &[&str],
    units: &Option<String>,
    doc: &str,
    gf: Option<&GraphicalFunction>,
) {
    let dim_suffix = if dims.is_empty() {
        String::new()
    } else {
        let dim_strs: Vec<String> = dims.iter().map(|d| format_mdl_ident(d)).collect();
        format!("[{}]", dim_strs.join(","))
    };

    if let Some(gf) = gf {
        if is_lookup_only_equation(eqn) {
            // Standalone lookup definition: name(\n\tbody)
            write!(buf, "{name}{dim_suffix}(").unwrap();
            buf.push_str("\n\t");
            write_lookup_body(buf, gf);
            buf.push(')');
        } else {
            // Embedded lookup: name=\n\tWITH LOOKUP(input, (body))
            let assign_op = if is_data_equation(eqn) { ":=" } else { "=" };
            write!(buf, "{name}{dim_suffix}{assign_op}").unwrap();
            let mdl_eqn = equation_to_mdl(eqn);
            buf.push_str("\n\tWITH LOOKUP(");
            buf.push_str(&mdl_eqn);
            buf.push_str(", ");
            write_lookup(buf, gf);
            buf.push(')');
        }
    } else {
        let assign_op = if is_data_equation(eqn) { ":=" } else { "=" };
        let mdl_eqn = equation_to_mdl(eqn);

        // Short, single-line equations use inline format with spaces around
        // the operator (e.g. `average repayment rate = 0.03`).  Longer or
        // multiline equations use the traditional Vensim multiline format.
        let inline_line = format!("{name}{dim_suffix} {assign_op} {mdl_eqn}");
        if inline_line.len() <= 80 && !mdl_eqn.contains('\n') {
            buf.push_str(&inline_line);
        } else {
            write!(buf, "{name}{dim_suffix}{assign_op}").unwrap();
            let wrapped = wrap_equation_with_continuations(&mdl_eqn, 80);
            buf.push_str("\n\t");
            buf.push_str(&wrapped);
        }
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
    name: &str,
    _dims: &[String],
    elements: &[(String, String, Option<String>, Option<GraphicalFunction>)],
    _default_equation: &Option<String>,
    units: &Option<String>,
    doc: &str,
) {
    write_arrayed_element_entries(buf, name, elements, units, doc);
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

        if let Some(gf) = elem_gf {
            if is_lookup_only_equation(eqn) {
                write!(buf, "{name}[{elem_display}](").unwrap();
                buf.push_str("\n\t");
                write_lookup_body(buf, gf);
                buf.push(')');
            } else {
                let assign_op = if is_data_equation(eqn) { ":=" } else { "=" };
                write!(buf, "{name}[{elem_display}]{assign_op}").unwrap();
                let mdl_eqn = equation_to_mdl(eqn);
                buf.push_str("\n\tWITH LOOKUP(");
                buf.push_str(&mdl_eqn);
                buf.push_str(", ");
                write_lookup(buf, gf);
                buf.push(')');
            }
        } else {
            let assign_op = if is_data_equation(eqn) { ":=" } else { "=" };
            write!(buf, "{name}[{elem_display}]{assign_op}").unwrap();
            let mdl_eqn = equation_to_mdl(eqn);
            let wrapped = wrap_equation_with_continuations(&mdl_eqn, 80);
            buf.push_str("\n\t");
            buf.push_str(&wrapped);
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
                .map(|(i, e)| (to_lower_space(e), i))
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

/// Format a view element name for sketch output.
///
/// Sketch names need underscores replaced with spaces (like `underbar_to_space`),
/// but also must escape actual newline characters as the literal two-character
/// sequence `\n`. XMILE sources may contain real newlines in view element name
/// attributes; Vensim MDL sketch lines are single-line records, so a real
/// newline in a name would break parsing.
fn format_sketch_name(name: &str) -> String {
    name.replace('_', " ").replace('\n', "\\n")
}

/// Remap merged/global datamodel UIDs into dense, view-local sketch IDs.
///
/// Vensim sketches use small, contiguous IDs within each `V300` section.
/// After multi-view MDL files are merged into a single StockFlow, the
/// datamodel UIDs remain globally unique across the merged view. Re-using
/// those sparse IDs when serializing a single segment produces valid-looking
/// records that Vensim misrenders. The writer therefore assigns fresh,
/// per-segment IDs while leaving geometry lookups on the original IDs.
struct SketchUidRemap {
    element_uids: HashMap<i32, i32>,
    valve_uids: HashMap<i32, i32>,
}

impl SketchUidRemap {
    fn dense_for_segment(elements: &[&ViewElement]) -> Self {
        let mut element_uids = HashMap::new();
        let mut flow_uids = Vec::new();
        let mut next_uid = 1;

        for element in elements {
            let old_uid = match element {
                ViewElement::Aux(aux) => aux.uid,
                ViewElement::Stock(stock) => stock.uid,
                ViewElement::Flow(flow) => {
                    flow_uids.push(flow.uid);
                    flow.uid
                }
                ViewElement::Link(link) => link.uid,
                ViewElement::Alias(alias) => alias.uid,
                ViewElement::Cloud(cloud) => cloud.uid,
                ViewElement::Module(_) | ViewElement::Group(_) => continue,
            };
            element_uids.insert(old_uid, next_uid);
            next_uid += 1;
        }

        let mut valve_uids = HashMap::new();
        for flow_uid in flow_uids {
            valve_uids.insert(flow_uid, next_uid);
            next_uid += 1;
        }

        Self {
            element_uids,
            valve_uids,
        }
    }

    fn element_uid(&self, old_uid: i32) -> i32 {
        self.element_uids.get(&old_uid).copied().unwrap_or(old_uid)
    }

    fn valve_uid(&self, flow_uid: i32) -> Option<i32> {
        self.valve_uids.get(&flow_uid).copied()
    }

    fn next_connector_uid(&self) -> i32 {
        (self.element_uids.len() + self.valve_uids.len()) as i32 + 1
    }
}

const STOCK_WIDTH: f64 = 45.0;
const STOCK_HEIGHT: f64 = 35.0;
const STOCK_EDGE_TOLERANCE: f64 = 1.0;

/// Write a type 10 line for an Aux element.
/// Sketch element names use `format_sketch_name` (not `format_mdl_ident`)
/// because MDL sketch lines are comma-delimited positional records where
/// quoting is not used.
#[cfg(test)]
fn write_aux_element(buf: &mut String, aux: &view_element::Aux) {
    write_aux_element_with_context(buf, aux, SketchTransform::identity(), None);
}

fn write_aux_element_with_context(
    buf: &mut String,
    aux: &view_element::Aux,
    transform: SketchTransform,
    uid_remap: Option<&SketchUidRemap>,
) {
    let name = format_sketch_name(&aux.name);
    let (w, h, shape, bits) = match &aux.compat {
        Some(c) => (c.width as i32, c.height as i32, c.shape, c.bits),
        None => (40, 20, 8, 3),
    };
    let (x, y) = transform.point(aux.x, aux.y);
    let tail = compat_tail(aux.compat.as_ref(), "0,0,-1,0,0,0");
    let uid = uid_remap.map_or(aux.uid, |ids| ids.element_uid(aux.uid));
    write!(
        buf,
        "10,{},{},{},{},{},{},{},{},{}",
        uid, name, x, y, w, h, shape, bits, tail,
    )
    .unwrap();
}

/// Write a type 10 line for a Stock element.
#[cfg(test)]
fn write_stock_element(buf: &mut String, stock: &view_element::Stock) {
    write_stock_element_with_context(buf, stock, SketchTransform::identity(), None);
}

fn write_stock_element_with_context(
    buf: &mut String,
    stock: &view_element::Stock,
    transform: SketchTransform,
    uid_remap: Option<&SketchUidRemap>,
) {
    let name = format_sketch_name(&stock.name);
    let (w, h, shape, bits) = match &stock.compat {
        Some(c) => (c.width as i32, c.height as i32, c.shape, c.bits),
        None => (40, 20, 3, 3),
    };
    let (x, y) = transform.point(stock.x, stock.y);
    let tail = compat_tail(stock.compat.as_ref(), "0,0,0,0,0,0");
    let uid = uid_remap.map_or(stock.uid, |ids| ids.element_uid(stock.uid));
    write!(
        buf,
        "10,{},{},{},{},{},{},{},{},{}",
        uid, name, x, y, w, h, shape, bits, tail,
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

#[allow(dead_code)]
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

#[derive(Clone, Copy)]
struct SketchTransform {
    x_offset: f64,
    y_offset: f64,
}

impl SketchTransform {
    fn identity() -> Self {
        Self {
            x_offset: 0.0,
            y_offset: 0.0,
        }
    }

    fn point(self, x: f64, y: f64) -> (i32, i32) {
        (
            (x - self.x_offset).round() as i32,
            (y - self.y_offset).round() as i32,
        )
    }
}

fn compat_tail<'a>(
    compat: Option<&'a view_element::ViewElementCompat>,
    default: &'a str,
) -> &'a str {
    compat.and_then(|c| c.tail.as_deref()).unwrap_or(default)
}

fn compat_name_field<'a>(
    compat: Option<&'a view_element::ViewElementCompat>,
    default: &'a str,
) -> &'a str {
    compat
        .and_then(|c| c.name_field.as_deref())
        .unwrap_or(default)
}

fn default_flow_label_point(flow: &view_element::Flow, transform: SketchTransform) -> (i32, i32) {
    let (x, y) = match flow.label_side {
        view_element::LabelSide::Top => (flow.x, flow.y - 16.0),
        view_element::LabelSide::Left => (flow.x - 16.0, flow.y),
        view_element::LabelSide::Center => (flow.x, flow.y),
        view_element::LabelSide::Bottom => (flow.x, flow.y + 16.0),
        view_element::LabelSide::Right => (flow.x + 16.0, flow.y),
    };
    transform.point(x, y)
}

/// Write a Flow element as type 1 pipe connectors, type 11 (valve), and
/// type 10 (attached flow variable).
///
/// Vensim requires this exact ordering: pipe connectors first, then valve,
/// then flow label. The valve UID is looked up from the pre-allocated
/// valve_uids map to avoid collisions.
#[cfg(test)]
fn write_flow_element(
    buf: &mut String,
    flow: &view_element::Flow,
    valve_uids: &HashMap<i32, i32>,
    cloud_uids: &HashSet<i32>,
    next_connector_uid: &mut i32,
) {
    write_flow_element_with_context(
        buf,
        flow,
        valve_uids,
        cloud_uids,
        next_connector_uid,
        SketchTransform::identity(),
        &HashMap::new(),
        &HashSet::new(),
        None,
    );
}

#[allow(clippy::too_many_arguments)]
fn write_flow_element_with_context(
    buf: &mut String,
    flow: &view_element::Flow,
    valve_uids: &HashMap<i32, i32>,
    _cloud_uids: &HashSet<i32>,
    next_connector_uid: &mut i32,
    transform: SketchTransform,
    elem_positions: &HashMap<i32, (i32, i32)>,
    stock_uids: &HashSet<i32>,
    uid_remap: Option<&SketchUidRemap>,
) {
    let name = format_sketch_name(&flow.name);
    let valve_uid = uid_remap
        .and_then(|ids| ids.valve_uid(flow.uid))
        .or_else(|| valve_uids.get(&flow.uid).copied())
        .unwrap_or(flow.uid - 1);
    let valve_compat = flow.compat.as_ref();
    let label_compat = flow.label_compat.as_ref();
    let (valve_x, valve_y) = transform.point(flow.x, flow.y);

    // Pipe connectors must come before the valve and flow label.
    let had_pipes = write_flow_pipe_connectors_with_context(
        buf,
        flow,
        valve_uid,
        next_connector_uid,
        FlowConnectorContext {
            transform,
            elem_positions,
            stock_uids,
            uid_remap,
        },
    );

    let (valve_w, valve_h, valve_shape, valve_bits) = match valve_compat {
        Some(c) => (c.width as i32, c.height as i32, c.shape, c.bits),
        None => (6, 8, 34, 3),
    };
    let (label_w, label_h, label_shape, label_bits) = match label_compat {
        Some(c) => (c.width as i32, c.height as i32, c.shape, c.bits),
        None => (49, 8, 40, 3),
    };
    let valve_name = compat_name_field(valve_compat, "0");
    let valve_tail = compat_tail(valve_compat, "0,0,1,0,0,0");

    if had_pipes {
        buf.push('\n');
    }
    write!(
        buf,
        "11,{},{},{},{},{},{},{},{},{}",
        valve_uid,
        valve_name,
        valve_x,
        valve_y,
        valve_w,
        valve_h,
        valve_shape,
        valve_bits,
        valve_tail,
    )
    .unwrap();

    let (label_x, label_y) = default_flow_label_point(flow, transform);
    let label_tail = compat_tail(label_compat, "0,0,-1,0,0,0");
    let label_uid = uid_remap.map_or(flow.uid, |ids| ids.element_uid(flow.uid));
    write!(
        buf,
        "\n10,{},{},{},{},{},{},{},{},{}",
        label_uid, name, label_x, label_y, label_w, label_h, label_shape, label_bits, label_tail,
    )
    .unwrap();
}

/// Returns true if any pipe connectors were written.
#[cfg(test)]
#[allow(dead_code)]
fn write_flow_pipe_connectors(
    buf: &mut String,
    flow: &view_element::Flow,
    valve_uid: i32,
    _cloud_uids: &HashSet<i32>,
    next_connector_uid: &mut i32,
) -> bool {
    write_flow_pipe_connectors_with_context(
        buf,
        flow,
        valve_uid,
        next_connector_uid,
        FlowConnectorContext {
            transform: SketchTransform::identity(),
            elem_positions: &HashMap::new(),
            stock_uids: &HashSet::new(),
            uid_remap: None,
        },
    )
}

struct FlowConnectorContext<'a> {
    transform: SketchTransform,
    elem_positions: &'a HashMap<i32, (i32, i32)>,
    stock_uids: &'a HashSet<i32>,
    uid_remap: Option<&'a SketchUidRemap>,
}

fn write_flow_pipe_connectors_with_context(
    buf: &mut String,
    flow: &view_element::Flow,
    valve_uid: i32,
    next_connector_uid: &mut i32,
    ctx: FlowConnectorContext<'_>,
) -> bool {
    let mut wrote_any = false;

    // Flow pipe connectors use field 4 for endpoint type and field 7 = 22 (pipe type).
    // Flag 4 = connects to a stock, flag 100 = connects to a cloud.
    let write_pipe = |buf: &mut String,
                      first: bool,
                      connector_uid: i32,
                      from_uid: i32,
                      to_uid: i32,
                      direction: i32,
                      x: i32,
                      y: i32| {
        if !first {
            buf.push('\n');
        }
        write!(
            buf,
            "1,{},{},{},{},0,0,22,0,0,0,-1--1--1,,1|({},{})|",
            connector_uid, from_uid, to_uid, direction, x, y,
        )
        .unwrap();
    };

    let connector_point = |point: &view_element::FlowPoint| -> (i32, i32) {
        let point_xy = ctx.transform.point(point.x, point.y);
        let Some(endpoint_uid) = point.attached_to_uid else {
            return point_xy;
        };
        if !ctx.stock_uids.contains(&endpoint_uid) {
            return point_xy;
        }

        let Some(&(stock_x, stock_y)) = ctx.elem_positions.get(&endpoint_uid) else {
            return point_xy;
        };
        let dx = f64::from(point_xy.0 - stock_x);
        let dy = f64::from(point_xy.1 - stock_y);
        let on_left_or_right = (dx.abs() - STOCK_WIDTH / 2.0).abs() <= STOCK_EDGE_TOLERANCE
            && dy.abs() <= STOCK_HEIGHT / 2.0 + STOCK_EDGE_TOLERANCE;
        if on_left_or_right {
            return (stock_x, point_xy.1);
        }

        let on_top_or_bottom = (dy.abs() - STOCK_HEIGHT / 2.0).abs() <= STOCK_EDGE_TOLERANCE
            && dx.abs() <= STOCK_WIDTH / 2.0 + STOCK_EDGE_TOLERANCE;
        if on_top_or_bottom {
            return (point_xy.0, stock_y);
        }

        point_xy
    };

    if flow.points.len() > 1
        && let Some(last) = flow.points.last()
        && let Some(endpoint_uid) = last.attached_to_uid
    {
        let (x, y) = connector_point(last);
        let direction = if ctx.stock_uids.contains(&endpoint_uid) {
            4
        } else {
            100
        };
        let endpoint_uid = ctx
            .uid_remap
            .map_or(endpoint_uid, |ids| ids.element_uid(endpoint_uid));
        write_pipe(
            buf,
            !wrote_any,
            *next_connector_uid,
            valve_uid,
            endpoint_uid,
            direction,
            x,
            y,
        );
        wrote_any = true;
        *next_connector_uid += 1;
    }

    for point in flow
        .points
        .iter()
        .skip(1)
        .take(flow.points.len().saturating_sub(2))
    {
        let (x, y) = connector_point(point);
        write_pipe(
            buf,
            !wrote_any,
            *next_connector_uid,
            valve_uid,
            valve_uid,
            0,
            x,
            y,
        );
        wrote_any = true;
        *next_connector_uid += 1;
    }

    if let Some(first) = flow.points.first()
        && let Some(endpoint_uid) = first.attached_to_uid
    {
        let (x, y) = connector_point(first);
        let direction = if ctx.stock_uids.contains(&endpoint_uid) {
            4
        } else {
            100
        };
        let endpoint_uid = ctx
            .uid_remap
            .map_or(endpoint_uid, |ids| ids.element_uid(endpoint_uid));
        write_pipe(
            buf,
            !wrote_any,
            *next_connector_uid,
            valve_uid,
            endpoint_uid,
            direction,
            x,
            y,
        );
        wrote_any = true;
        *next_connector_uid += 1;
    }

    wrote_any
}

/// Write a type 12 line for a Cloud element.
#[cfg(test)]
fn write_cloud_element(buf: &mut String, cloud: &view_element::Cloud) {
    write_cloud_element_with_context(buf, cloud, SketchTransform::identity(), None);
}

fn write_cloud_element_with_context(
    buf: &mut String,
    cloud: &view_element::Cloud,
    transform: SketchTransform,
    uid_remap: Option<&SketchUidRemap>,
) {
    let (w, h, shape, bits) = match &cloud.compat {
        Some(c) => (c.width as i32, c.height as i32, c.shape, c.bits),
        None => (10, 8, 0, 3),
    };
    let (x, y) = transform.point(cloud.x, cloud.y);
    let name_field = compat_name_field(cloud.compat.as_ref(), "48");
    let tail = compat_tail(cloud.compat.as_ref(), "0,0,-1,0,0,0");
    let uid = uid_remap.map_or(cloud.uid, |ids| ids.element_uid(cloud.uid));
    write!(
        buf,
        "12,{},{},{},{},{},{},{},{},{}",
        uid, name_field, x, y, w, h, shape, bits, tail,
    )
    .unwrap();
}

/// Write a type 10 line for an Alias (ghost) element.
#[cfg(test)]
fn write_alias_element(
    buf: &mut String,
    alias: &view_element::Alias,
    name_map: &HashMap<i32, &str>,
) {
    write_alias_element_with_context(
        buf,
        alias,
        name_map,
        &HashSet::new(),
        SketchTransform::identity(),
        None,
    );
}

fn write_alias_element_with_context(
    buf: &mut String,
    alias: &view_element::Alias,
    name_map: &HashMap<i32, &str>,
    stock_uids: &HashSet<i32>,
    transform: SketchTransform,
    uid_remap: Option<&SketchUidRemap>,
) {
    let name = name_map
        .get(&alias.alias_of_uid)
        .map(|n| format_sketch_name(n))
        .unwrap_or_default();
    let (w, h, shape, bits) = match &alias.compat {
        Some(c) => (c.width as i32, c.height as i32, c.shape, c.bits),
        None => (40, 20, 8, 2),
    };
    let (alias_x, alias_y) = if stock_uids.contains(&alias.alias_of_uid) {
        (alias.x + 22.0, alias.y + 17.0)
    } else {
        (alias.x, alias.y)
    };
    let (x, y) = transform.point(alias_x, alias_y);
    let tail = compat_tail(
        alias.compat.as_ref(),
        "0,3,-1,0,0,0,128-128-128,0-0-0,|12||128-128-128",
    );
    let uid = uid_remap.map_or(alias.uid, |ids| ids.element_uid(alias.uid));
    // shape=8
    write!(
        buf,
        "10,{},{},{},{},{},{},{},{},{}",
        uid, name, x, y, w, h, shape, bits, tail,
    )
    .unwrap();
}

/// Write a type 1 line for a Link (connector) element.
///
/// For arc connectors, we reverse-compute a control point from the stored
/// canvas angle using the endpoints of the connected elements.
#[cfg(test)]
fn write_link_element(
    buf: &mut String,
    link: &view_element::Link,
    elem_positions: &HashMap<i32, (i32, i32)>,
    use_lettered_polarity: bool,
) {
    write_link_element_with_context(
        buf,
        link,
        elem_positions,
        use_lettered_polarity,
        None,
        SketchTransform::identity(),
        None,
    );
}

fn write_link_element_with_context(
    buf: &mut String,
    link: &view_element::Link,
    elem_positions: &HashMap<i32, (i32, i32)>,
    use_lettered_polarity: bool,
    link_compat: Option<&view_element::LinkSketchCompat>,
    transform: SketchTransform,
    uid_remap: Option<&SketchUidRemap>,
) {
    let polarity_val = match link.polarity {
        Some(LinkPolarity::Positive) if use_lettered_polarity => 83, // 'S'
        Some(LinkPolarity::Negative) if use_lettered_polarity => 79, // 'O'
        Some(LinkPolarity::Positive) => 43,                          // '+'
        Some(LinkPolarity::Negative) => 45,                          // '-'
        None => 0,
    };

    let from_uid = link.from_uid;
    let to_uid = link.to_uid;
    let from_pos = elem_positions.get(&from_uid).copied().unwrap_or((0, 0));
    let to_pos = elem_positions.get(&to_uid).copied().unwrap_or((0, 0));
    let link_uid = uid_remap.map_or(link.uid, |ids| ids.element_uid(link.uid));
    let from_uid = uid_remap.map_or(from_uid, |ids| ids.element_uid(from_uid));
    let to_uid = uid_remap.map_or(to_uid, |ids| ids.element_uid(to_uid));
    let field4 = link_compat.map(|compat| compat.field4).unwrap_or(0);
    let field10 = link_compat.map(|compat| compat.field10).unwrap_or(0);

    // Field 9 = 64 marks influence (causal) connectors in Vensim sketches.
    match &link.shape {
        LinkShape::Straight => {
            write!(
                buf,
                "1,{},{},{},{},0,{},0,0,64,{},-1--1--1,,1|(0,0)|",
                link_uid, from_uid, to_uid, field4, polarity_val, field10,
            )
            .unwrap();
        }
        LinkShape::Arc(canvas_angle) => {
            let (ctrl_x, ctrl_y) = compute_control_point(from_pos, to_pos, *canvas_angle);
            write!(
                buf,
                "1,{},{},{},{},0,{},0,0,64,{},-1--1--1,,1|({},{})|",
                link_uid, from_uid, to_uid, field4, polarity_val, field10, ctrl_x, ctrl_y,
            )
            .unwrap();
        }
        LinkShape::MultiPoint(points) => {
            let npoints = points.len();
            write!(
                buf,
                "1,{},{},{},{},0,{},0,0,64,{},-1--1--1,,{}|",
                link_uid, from_uid, to_uid, field4, polarity_val, field10, npoints,
            )
            .unwrap();
            for pt in points {
                let (x, y) = transform.point(pt.x, pt.y);
                write!(buf, "({},{})|", x, y).unwrap();
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

/// Splits a StockFlow's elements into view segments at MDL view-marker
/// Group boundaries.
///
/// When the MDL parser merges multiple named views into a single StockFlow,
/// it inserts a Group element (with `is_mdl_view_marker == true`) at the
/// start of each original view's elements.  This function reverses that
/// merge by splitting on those markers.  Organizational groups from XMILE
/// (where `is_mdl_view_marker == false`) are passed through as regular
/// elements rather than triggering a view split.
///
/// Returns a Vec of (view_name, elements, font). If no marker Groups exist,
/// returns a single segment using the StockFlow's own name (or "View 1").
fn split_view_on_groups<'a>(
    sf: &'a datamodel::StockFlow,
) -> Vec<(String, Vec<&'a ViewElement>, Option<String>)> {
    let has_mdl_markers = sf.elements.iter().any(|e| {
        matches!(
            e,
            ViewElement::Group(g) if g.is_mdl_view_marker
        )
    });

    if !has_mdl_markers {
        let name = sf.name.clone().unwrap_or_else(|| "View 1".to_string());
        let elements: Vec<&ViewElement> = sf
            .elements
            .iter()
            .filter(|e| !matches!(e, ViewElement::Module(_)))
            .collect();
        return vec![(name, elements, sf.font.clone())];
    }

    let mut segments = Vec::new();
    let mut current_name = sf.name.clone().unwrap_or_else(|| "View 1".to_string());
    let mut current_elements: Vec<&'a ViewElement> = Vec::new();

    let mut seen_marker = false;
    for element in &sf.elements {
        if let ViewElement::Group(group) = element
            && group.is_mdl_view_marker
        {
            // Push the previous segment. Skip the initial pre-Group segment
            // only if it has no elements (no content before the first Group).
            if seen_marker || !current_elements.is_empty() {
                segments.push((current_name, current_elements, sf.font.clone()));
                current_elements = Vec::new();
            }
            seen_marker = true;
            current_name = group.name.clone();
            continue;
        }
        if !matches!(element, ViewElement::Module(_)) {
            current_elements.push(element);
        }
    }
    // Push the final segment (may be empty for trailing Groups).
    if seen_marker || !current_elements.is_empty() {
        segments.push((current_name, current_elements, sf.font.clone()));
    }
    segments
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
    ///
    /// Vensim requires CRLF (`\r\n`) line endings, so the final output
    /// is converted from LF to CRLF before returning.
    pub(super) fn write_project(mut self, project: &datamodel::Project) -> Result<String> {
        self.buf.push_str("{UTF-8}\n");
        let model = &project.models[0];
        self.write_equations_section(model, project);
        self.write_sketch_section(&model.views);
        self.write_settings_section(project);
        Ok(self.buf.replace('\n', "\r\n"))
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

        let display_names = build_display_name_map(&model.views);

        // Build a set of variable idents that belong to any group
        // (skip .Control -- those vars are sim specs emitted separately)
        let mut grouped_idents: HashSet<&str> = HashSet::new();
        for group in &model.groups {
            if group.name.eq_ignore_ascii_case("Control") {
                continue;
            }
            for member in &group.members {
                grouped_idents.insert(member.as_str());
            }
        }

        // 2. Variables in group order (skip .Control -- emitted with sim specs)
        for group in &model.groups {
            if group.name.eq_ignore_ascii_case("Control") {
                continue;
            }
            // Group marker
            write!(
                self.buf,
                "\n********************************************************\n\t.{}\n********************************************************~\n\t\t{}\n\t|\n",
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
                    write_variable_entry(&mut self.buf, var, &display_names);
                    self.buf.push('\n');
                }
            }
        }

        // 3. Ungrouped variables (alphabetical by ident for deterministic output)
        let mut ungrouped: Vec<&datamodel::Variable> = model
            .variables
            .iter()
            .filter(|v| !grouped_idents.contains(v.get_ident()))
            .collect();
        ungrouped.sort_by_key(|v| v.get_ident());

        for var in ungrouped {
            write_variable_entry(&mut self.buf, var, &display_names);
            self.buf.push('\n');
        }

        // 4. .Control group header + sim spec variables
        self.buf.push_str(
            "\n********************************************************\n\t.Control\n********************************************************~\n\t\tSimulation Control Parameters\n\t|\n",
        );
        let sim_specs = model.sim_specs.as_ref().unwrap_or(&project.sim_specs);
        self.write_sim_specs(sim_specs);

        // 5. Section terminator
        self.buf
            .push_str("\\\\\\---/// Sketch information - do not modify anything except names\n");
    }

    /// Write the sketch/view section of the MDL file.
    ///
    /// Each view gets its own `\\\---///` separator and `V300` header line.
    /// The first view's separator is already emitted by `write_equations_section`.
    /// The final `///---\\\` terminator follows the last view.
    ///
    /// When a StockFlow contains Group elements (from merging multiple MDL
    /// views at parse time), we split on those boundaries to reconstruct
    /// the original multi-view structure.
    fn write_sketch_section(&mut self, views: &[View]) {
        if views.is_empty() {
            // Emit a minimal valid sketch so the output is not malformed.
            self.buf
                .push_str("V300  Do not put anything below this section - it will be ignored\n");
            self.buf.push_str("*View 1\n");
            self.buf
                .push_str("$192-192-192,0,Times New Roman|12||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|96,96,100,0\n");
            self.buf.push_str("///---\\\\\\\n");
            return;
        }

        let mut segment_idx = 0;
        for view in views {
            let View::StockFlow(sf) = view;

            // Build shared maps from ALL elements so that cross-view
            // references (links, aliases) resolve correctly.
            let valve_uids = allocate_valve_uids(&sf.elements);
            let name_map = build_name_map(&sf.elements);
            let mut flow_compat_by_uid: HashMap<i32, &view_element::FlowSketchCompat> =
                HashMap::new();
            let mut link_compat_by_uid: HashMap<i32, &view_element::LinkSketchCompat> =
                HashMap::new();
            let mut stock_uids: HashSet<i32> = HashSet::new();
            if let Some(sketch_compat) = sf.sketch_compat.as_ref() {
                for flow in &sketch_compat.flows {
                    flow_compat_by_uid.insert(flow.uid, flow);
                }
                for link in &sketch_compat.links {
                    link_compat_by_uid.insert(link.uid, link);
                }
            }
            for elem in &sf.elements {
                if let ViewElement::Stock(stock) = elem {
                    stock_uids.insert(stock.uid);
                }
            }

            let segments = split_view_on_groups(sf);
            let mut elem_positions = HashMap::new();
            for (segment_ix, (_, elements, _)) in segments.iter().enumerate() {
                let transform = sf
                    .sketch_compat
                    .as_ref()
                    .and_then(|compat| compat.segments.get(segment_ix))
                    .map(|compat| SketchTransform {
                        x_offset: compat.x_offset,
                        y_offset: compat.y_offset,
                    })
                    .unwrap_or_else(SketchTransform::identity);
                elem_positions.extend(build_element_positions_with_transform(
                    elements,
                    &valve_uids,
                    transform,
                    &stock_uids,
                    &flow_compat_by_uid,
                ));
            }

            for (segment_ix, (view_name, elements, font)) in segments.iter().enumerate() {
                if segment_idx > 0 {
                    self.buf.push_str(
                        "\\\\\\---/// Sketch information - do not modify anything except names\n",
                    );
                }
                self.buf.push_str(
                    "V300  Do not put anything below this section - it will be ignored\n",
                );
                self.write_view_segment(
                    view_name,
                    elements,
                    font.as_deref(),
                    sf.use_lettered_polarity,
                    sf.sketch_compat
                        .as_ref()
                        .and_then(|compat| compat.segments.get(segment_ix))
                        .map(|compat| SketchTransform {
                            x_offset: compat.x_offset,
                            y_offset: compat.y_offset,
                        })
                        .unwrap_or_else(SketchTransform::identity),
                    &elem_positions,
                    &name_map,
                    &stock_uids,
                    &flow_compat_by_uid,
                    &link_compat_by_uid,
                );
                segment_idx += 1;
            }
        }

        self.buf.push_str("///---\\\\\\\n");
    }

    /// Write a single view segment: title, font line, and all sketch elements.
    #[allow(clippy::too_many_arguments)]
    fn write_view_segment(
        &mut self,
        view_name: &str,
        elements: &[&ViewElement],
        font: Option<&str>,
        use_lettered_polarity: bool,
        transform: SketchTransform,
        elem_positions: &HashMap<i32, (i32, i32)>,
        name_map: &HashMap<i32, &str>,
        stock_uids: &HashSet<i32>,
        _flow_compat_by_uid: &HashMap<i32, &view_element::FlowSketchCompat>,
        link_compat_by_uid: &HashMap<i32, &view_element::LinkSketchCompat>,
    ) {
        let uid_remap = SketchUidRemap::dense_for_segment(elements);
        let mut next_connector_uid = uid_remap.next_connector_uid();
        let view_title = sanitize_view_title_for_mdl(view_name);
        writeln!(self.buf, "*{}", view_title).unwrap();

        if let Some(f) = font {
            writeln!(self.buf, "${}", f).unwrap();
        } else {
            self.buf.push_str(
                "$192-192-192,0,Times New Roman|12||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|96,96,100,0\n",
            );
        }

        // Collect cloud UIDs so flow pipe connectors can set the right direction flag.
        // Also build a map from flow_uid -> clouds so we can emit each cloud
        // just before its associated flow (Vensim requires this ordering).
        let mut cloud_uids: HashSet<i32> = HashSet::new();
        let mut flow_clouds: HashMap<i32, Vec<&view_element::Cloud>> = HashMap::new();
        for elem in elements {
            if let ViewElement::Cloud(c) = *elem {
                cloud_uids.insert(c.uid);
                flow_clouds.entry(c.flow_uid).or_default().push(c);
            }
        }

        for elem in elements {
            match elem {
                ViewElement::Aux(aux) => {
                    write_aux_element_with_context(&mut self.buf, aux, transform, Some(&uid_remap));
                    self.buf.push('\n');
                }
                ViewElement::Stock(stock) => {
                    write_stock_element_with_context(
                        &mut self.buf,
                        stock,
                        transform,
                        Some(&uid_remap),
                    );
                    self.buf.push('\n');
                }
                ViewElement::Flow(flow) => {
                    // Emit associated clouds before the flow pipes
                    if let Some(clouds) = flow_clouds.get(&flow.uid) {
                        for cloud in clouds {
                            write_cloud_element_with_context(
                                &mut self.buf,
                                cloud,
                                transform,
                                Some(&uid_remap),
                            );
                            self.buf.push('\n');
                        }
                    }
                    write_flow_element_with_context(
                        &mut self.buf,
                        flow,
                        &uid_remap.valve_uids,
                        &cloud_uids,
                        &mut next_connector_uid,
                        transform,
                        elem_positions,
                        stock_uids,
                        Some(&uid_remap),
                    );
                    self.buf.push('\n');
                }
                ViewElement::Link(link) => {
                    write_link_element_with_context(
                        &mut self.buf,
                        link,
                        elem_positions,
                        use_lettered_polarity,
                        link_compat_by_uid.get(&link.uid).copied(),
                        transform,
                        Some(&uid_remap),
                    );
                    self.buf.push('\n');
                }
                // Clouds are emitted with their associated flow above
                ViewElement::Cloud(_) => {}
                ViewElement::Alias(alias) => {
                    write_alias_element_with_context(
                        &mut self.buf,
                        alias,
                        name_map,
                        stock_uids,
                        transform,
                        Some(&uid_remap),
                    );
                    self.buf.push('\n');
                }
                ViewElement::Module(_) | ViewElement::Group(_) => {}
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

        // The ///---\\\ separator is already emitted by write_sketch_section.
        // The 0x7F (DEL) between :L and <%^E!@ is required by Vensim's parser.
        self.buf.push_str(":L\x7F<%^E!@\n");

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

        // Types 24/25/26: Display time range for the graph/chart output.
        // These control what Vensim shows in its default output graphs,
        // NOT the simulation time range (which comes from the TIME STEP,
        // INITIAL TIME, and FINAL TIME variable definitions).
        // All reference MDL files set 24=start, 25=stop, 26=stop.
        writeln!(self.buf, "24:{}", format_f64(sim_specs.start)).unwrap();
        writeln!(self.buf, "25:{}", format_f64(sim_specs.stop)).unwrap();
        writeln!(self.buf, "26:{}", format_f64(sim_specs.stop)).unwrap();
    }
}

/// Build a map from element UID to (x, y) position for link control point computation.
///
/// For flow elements, `write_flow_element` emits a synthetic valve using the
/// pre-allocated `valve_uids` map. We register that valve UID here so that any
/// connector whose endpoint is the valve can resolve a position.
#[cfg(test)]
#[allow(dead_code)]
fn build_element_positions(
    elements: &[ViewElement],
    valve_uids: &HashMap<i32, i32>,
) -> HashMap<i32, (i32, i32)> {
    build_element_positions_with_transform(
        &elements.iter().collect::<Vec<_>>(),
        valve_uids,
        SketchTransform::identity(),
        &HashSet::new(),
        &HashMap::new(),
    )
}

fn build_element_positions_with_transform(
    elements: &[&ViewElement],
    valve_uids: &HashMap<i32, i32>,
    transform: SketchTransform,
    stock_uids: &HashSet<i32>,
    _flow_compat_by_uid: &HashMap<i32, &view_element::FlowSketchCompat>,
) -> HashMap<i32, (i32, i32)> {
    let mut positions = HashMap::new();
    for elem in elements {
        let (uid, x, y) = match elem {
            ViewElement::Aux(a) => {
                let (x, y) = transform.point(a.x, a.y);
                (a.uid, x, y)
            }
            ViewElement::Stock(s) => {
                let (x, y) = transform.point(s.x, s.y);
                (s.uid, x, y)
            }
            ViewElement::Flow(f) => {
                let (valve_x, valve_y) = transform.point(f.x, f.y);
                // Also register the allocated valve UID so connectors that
                // reference the valve position can resolve.
                if let Some(&valve_uid) = valve_uids.get(&f.uid) {
                    positions.insert(valve_uid, (valve_x, valve_y));
                }
                let (label_x, label_y) = default_flow_label_point(f, transform);
                (f.uid, label_x, label_y)
            }
            ViewElement::Cloud(c) => {
                let (x, y) = transform.point(c.x, c.y);
                (c.uid, x, y)
            }
            ViewElement::Alias(a) => {
                let (x, y) = if stock_uids.contains(&a.alias_of_uid) {
                    transform.point(a.x + 22.0, a.y + 17.0)
                } else {
                    transform.point(a.x, a.y)
                };
                (a.uid, x, y)
            }
            ViewElement::Module(m) => {
                let (x, y) = transform.point(m.x, m.y);
                (m.uid, x, y)
            }
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
#[path = "writer_tests.rs"]
mod tests;
