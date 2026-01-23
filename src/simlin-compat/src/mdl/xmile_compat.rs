// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! XMILE-compatible expression formatter.
//!
//! Converts MDL AST expressions to XMILE-compatible equation strings,
//! matching xmutil's `OutputComputable` behaviors including:
//! - Function renames and argument reordering
//! - Name formatting (spaces to underscores)
//! - Number formatting using %g style
//! - Operator formatting with proper spacing

use std::collections::HashSet;

use crate::mdl::ast::{BinaryOp, CallKind, Expr, LookupTable, Subscript, UnaryOp};
use crate::mdl::builtins::to_lower_space;

/// Formats MDL AST expressions as XMILE-compatible equation strings.
pub struct XmileFormatter {
    /// Whether to use TIME as STARTTIME reference
    use_xmile_time_names: bool,
    /// Canonical names of dimensions that are subranges (have maps_to set).
    /// Bang subscripts on these dimensions output "Dim.*" instead of just "*".
    subrange_dims: HashSet<String>,
}

impl Default for XmileFormatter {
    fn default() -> Self {
        Self::new()
    }
}

impl XmileFormatter {
    pub fn new() -> Self {
        XmileFormatter {
            use_xmile_time_names: true,
            subrange_dims: HashSet::new(),
        }
    }

    #[cfg(test)]
    pub fn with_subranges(subrange_dims: HashSet<String>) -> Self {
        XmileFormatter {
            use_xmile_time_names: true,
            subrange_dims,
        }
    }

    /// Set the subrange dimensions after construction.
    /// Called after dimensions are built to enable proper bang-subscript formatting.
    pub fn set_subranges(&mut self, subrange_dims: HashSet<String>) {
        self.subrange_dims = subrange_dims;
    }

    /// Format an expression to an XMILE-compatible string.
    pub fn format_expr(&self, expr: &Expr<'_>) -> String {
        self.format_expr_inner(expr)
    }

    fn format_expr_inner(&self, expr: &Expr<'_>) -> String {
        match expr {
            Expr::Const(value, _) => format_number(*value),
            Expr::Var(name, subscripts, _) => self.format_var(name, subscripts),
            Expr::App(name, subscripts, args, kind, _) => {
                self.format_call(name, subscripts, args, *kind)
            }
            Expr::Op1(op, inner, _) => self.format_unary(*op, inner),
            Expr::Op2(op, left, right, _) => self.format_binary(*op, left, right),
            Expr::Paren(inner, _) => format!("({})", self.format_expr_inner(inner)),
            Expr::Literal(lit, _) => {
                // Literals are already quoted in the AST, output as-is for XMILE
                // But xmutil strips quotes from literals in expression output
                lit.to_string()
            }
            Expr::Na(_) => ":NA:".to_string(),
        }
    }

    fn format_var(&self, name: &str, subscripts: &[Subscript<'_>]) -> String {
        let formatted_name = self.format_name(name);
        if subscripts.is_empty() {
            formatted_name
        } else {
            let subs: Vec<String> = subscripts
                .iter()
                .map(|s| match s {
                    Subscript::Element(n, _) => space_to_underbar(n),
                    // Bang subscript `dim!` means "iterate over all elements"
                    // For full dimensions -> `*`
                    // For subranges (have maps_to) -> `Dim.*`
                    Subscript::BangElement(n, _) => {
                        let canonical = to_lower_space(n);
                        if self.subrange_dims.contains(&canonical) {
                            format!("{}.*", space_to_underbar(n))
                        } else {
                            "*".to_string()
                        }
                    }
                })
                .collect();
            format!("{}[{}]", formatted_name, subs.join(", "))
        }
    }

    fn format_name(&self, name: &str) -> String {
        let canonical = to_lower_space(name);

        // Handle special TIME-related names
        if self.use_xmile_time_names {
            match canonical.as_str() {
                "time" => return "TIME".to_string(),
                "initial time" => return "STARTTIME".to_string(),
                "final time" => return "STOPTIME".to_string(),
                "time step" => return "DT".to_string(),
                "saveper" => return "SAVEPER".to_string(),
                _ => {}
            }
        }

        // Apply space-to-underbar transformation
        quoted_space_to_underbar(name)
    }

    fn format_call(
        &self,
        name: &str,
        subscripts: &[Subscript<'_>],
        args: &[Expr<'_>],
        kind: CallKind,
    ) -> String {
        let canonical = to_lower_space(name);

        // Handle special function transformations
        match canonical.as_str() {
            "a function of" => {
                // xmutil emits literal NAN, not NAN(args)
                return "NAN".to_string();
            }
            "if then else" => {
                if args.len() >= 3 {
                    return format!(
                        "( IF {} THEN {} ELSE {} )",
                        self.format_expr_inner(&args[0]),
                        self.format_expr_inner(&args[1]),
                        self.format_expr_inner(&args[2])
                    );
                }
            }
            "log" => {
                // LOG in Vensim: 1 arg = LOG10, 2 args = LOG(x, base) = LN(x)/LN(base)
                if args.len() == 1 {
                    return format!("LOG10({})", self.format_expr_inner(&args[0]));
                } else if args.len() == 2 {
                    return format!(
                        "(LN({}) / LN({}))",
                        self.format_expr_inner(&args[0]),
                        self.format_expr_inner(&args[1])
                    );
                }
            }
            "elmcount" => {
                if !args.is_empty() {
                    return format!("SIZE({})", self.format_expr_inner(&args[0]));
                }
            }
            "delay n" => {
                // DELAY N(input, dt, init, n) -> DELAYN(input, dt, n, init)
                if args.len() >= 4 {
                    return format!(
                        "DELAYN({}, {}, {}, {})",
                        self.format_expr_inner(&args[0]),
                        self.format_expr_inner(&args[1]),
                        self.format_expr_inner(&args[3]),
                        self.format_expr_inner(&args[2])
                    );
                }
            }
            "smooth n" => {
                // SMOOTH N(input, dt, init, n) -> SMTHN(input, dt, n, init)
                if args.len() >= 4 {
                    return format!(
                        "SMTHN({}, {}, {}, {})",
                        self.format_expr_inner(&args[0]),
                        self.format_expr_inner(&args[1]),
                        self.format_expr_inner(&args[3]),
                        self.format_expr_inner(&args[2])
                    );
                }
            }
            "random normal" => {
                // RANDOM NORMAL(min, max, mean, sd, seed) -> NORMAL(mean, sd, seed, min, max)
                if args.len() >= 5 {
                    return format!(
                        "NORMAL({}, {}, {}, {}, {})",
                        self.format_expr_inner(&args[2]),
                        self.format_expr_inner(&args[3]),
                        self.format_expr_inner(&args[4]),
                        self.format_expr_inner(&args[0]),
                        self.format_expr_inner(&args[1])
                    );
                }
            }
            "quantum" => {
                // QUANTUM(x, q) -> (q)*INT((x)/(q))
                if args.len() >= 2 {
                    let x = self.format_expr_inner(&args[0]);
                    let q = self.format_expr_inner(&args[1]);
                    return format!("({})*INT(({})/({}))", q, x, q);
                }
            }
            "pulse" => {
                // PULSE(start, width) -> IF TIME >= (start) AND TIME < ((start) + MAX(DT,width)) THEN 1 ELSE 0
                if args.len() >= 2 {
                    let start = self.format_expr_inner(&args[0]);
                    let width = self.format_expr_inner(&args[1]);
                    return format!(
                        "( IF TIME >= ({}) AND TIME < (({}) + MAX(DT,{})) THEN 1 ELSE 0 )",
                        start, start, width
                    );
                }
            }
            "pulse train" => {
                // PULSE TRAIN(start, width, interval, end) ->
                // IF TIME >= start AND TIME <= end AND (TIME - start) MOD interval < width THEN 1 ELSE 0
                // Note: Unlike PULSE which uses MAX(DT, width), PULSE TRAIN uses width directly (per xmutil)
                if args.len() >= 4 {
                    let start = self.format_expr_inner(&args[0]);
                    let width = self.format_expr_inner(&args[1]);
                    let interval = self.format_expr_inner(&args[2]);
                    let end = self.format_expr_inner(&args[3]);
                    return format!(
                        "( IF TIME >= ({}) AND TIME <= ({}) AND (TIME - ({})) MOD ({}) < ({}) THEN 1 ELSE 0 )",
                        start, end, start, interval, width
                    );
                }
            }
            "sample if true" => {
                // SAMPLE IF TRUE(cond, input, init) -> ( IF cond THEN input ELSE PREVIOUS(SELF, init) )
                if args.len() >= 3 {
                    return format!(
                        "( IF {} THEN {} ELSE PREVIOUS(SELF, {}) )",
                        self.format_expr_inner(&args[0]),
                        self.format_expr_inner(&args[1]),
                        self.format_expr_inner(&args[2])
                    );
                }
            }
            "allocate by priority" => {
                // ALLOCATE BY PRIORITY with reordered args
                return self.format_allocate_by_priority(args);
            }
            "random 0 1" => {
                // RANDOM 0 1() -> UNIFORM(0, 1)
                // Note: xmutil maps this to UNIFORM(0,1) with no additional args
                return "UNIFORM(0, 1)".to_string();
            }
            "random poisson" => {
                // RANDOM POISSON(min, max, mean, sdev, factor, seed)
                // -> POISSON((mean)/DT, seed, min, max) * factor + sdev
                // Note: xmutil uses arg[3] as offset and arg[4] as factor
                if args.len() >= 6 {
                    return format!(
                        "POISSON(({}) / DT, {}, {}, {}) * {} + {}",
                        self.format_expr_inner(&args[2]),
                        self.format_expr_inner(&args[5]),
                        self.format_expr_inner(&args[0]),
                        self.format_expr_inner(&args[1]),
                        self.format_expr_inner(&args[4]),
                        self.format_expr_inner(&args[3])
                    );
                }
            }
            "time base" => {
                // TIME BASE(t, dt) -> t + (dt) * TIME
                if args.len() >= 2 {
                    let t = self.format_expr_inner(&args[0]);
                    let dt = self.format_expr_inner(&args[1]);
                    return format!("{} + ({}) * TIME", t, dt);
                }
            }
            "zidz" => {
                // ZIDZ(a, b) -> SAFEDIV(a, b)
                if args.len() >= 2 {
                    return format!(
                        "SAFEDIV({}, {})",
                        self.format_expr_inner(&args[0]),
                        self.format_expr_inner(&args[1])
                    );
                }
            }
            "xidz" => {
                // XIDZ(a, b, x) -> SAFEDIV(a, b, x)
                if args.len() >= 3 {
                    return format!(
                        "SAFEDIV({}, {}, {})",
                        self.format_expr_inner(&args[0]),
                        self.format_expr_inner(&args[1]),
                        self.format_expr_inner(&args[2])
                    );
                }
            }
            _ => {}
        }

        // Check for lookup invocation (Symbol call with 1 arg)
        if kind == CallKind::Symbol && args.len() == 1 {
            let table_name = self.format_name(name);
            return format!(
                "LOOKUP({}, {})",
                table_name,
                self.format_expr_inner(&args[0])
            );
        }

        // Default function call formatting
        let func_name = self.format_function_name(&canonical);
        let formatted_args: Vec<String> = args.iter().map(|a| self.format_expr_inner(a)).collect();

        if subscripts.is_empty() {
            format!("{}({})", func_name, formatted_args.join(", "))
        } else {
            let subs: Vec<String> = subscripts
                .iter()
                .map(|s| match s {
                    Subscript::Element(n, _) => space_to_underbar(n),
                    // Bang subscript handling for function subscripts
                    Subscript::BangElement(n, _) => {
                        let canonical = to_lower_space(n);
                        if self.subrange_dims.contains(&canonical) {
                            format!("{}.*", space_to_underbar(n))
                        } else {
                            "*".to_string()
                        }
                    }
                })
                .collect();
            format!(
                "{}[{}]({})",
                func_name,
                subs.join(", "),
                formatted_args.join(", ")
            )
        }
    }

    fn format_function_name(&self, canonical: &str) -> String {
        // Map function names to XMILE equivalents
        match canonical {
            "a function of" => "".to_string(),
            "integ" => "INTEG".to_string(),
            "smooth" => "SMTH1".to_string(),
            "smoothi" => "SMTH1".to_string(),
            "smooth3" => "SMTH3".to_string(),
            "smooth3i" => "SMTH3".to_string(),
            "delay1" => "DELAY1".to_string(),
            "delay1i" => "DELAY1".to_string(),
            "delay3" => "DELAY3".to_string(),
            "delay3i" => "DELAY3".to_string(),
            "delay fixed" => "DELAY".to_string(),
            "active initial" => "INIT".to_string(),
            "initial" => "INIT".to_string(),
            "reinitial" => "INIT".to_string(),
            "integer" => "INT".to_string(),
            "lookup invert" => "LOOKUPINV".to_string(),
            "random uniform" => "UNIFORM".to_string(),
            "zidz" => "SAFEDIV".to_string(),
            "xidz" => "SAFEDIV".to_string(),
            "lookup extrapolate" => "LOOKUP".to_string(),
            "vmax" => "MAX".to_string(),
            "vmin" => "MIN".to_string(),
            "forecast" => "FORCST".to_string(),
            "random pink noise" => "NORMALPINK".to_string(),
            "vector select" => "VECTOR SELECT".to_string(),
            "vector elm map" => "VECTOR ELM MAP".to_string(),
            "vector sort order" => "VECTOR SORT ORDER".to_string(),
            "vector reorder" => "VECTOR_REORDER".to_string(),
            "vector lookup" => "VECTOR LOOKUP".to_string(),
            _ => canonical.to_uppercase().replace(' ', "_"),
        }
    }

    fn format_allocate_by_priority(&self, args: &[Expr<'_>]) -> String {
        // ALLOCATE BY PRIORITY(demand, priority, ignore, width, supply)
        // -> ALLOCATE(supply, last_subscript, demand_with_star, priority, width)
        if args.len() != 5 {
            // Fallback: pass through as-is
            let formatted: Vec<String> = args.iter().map(|a| self.format_expr_inner(a)).collect();
            return format!("ALLOCATE_BY_PRIORITY({})", formatted.join(", "));
        }

        let supply = self.format_expr_inner(&args[4]);
        let demand = &args[0];
        let priority = self.format_expr_inner(&args[1]);
        let width = self.format_expr_inner(&args[3]);

        // Extract last subscript from demand if it's a subscripted variable
        let (last_subscript, demand_str) = if let Expr::Var(name, subscripts, _) = demand {
            if subscripts.is_empty() {
                // No subscripts - use empty string for dimension, format normally
                (String::new(), self.format_name(name))
            } else {
                let last = subscripts
                    .last()
                    .map(|s| match s {
                        Subscript::Element(n, _) | Subscript::BangElement(n, _) => {
                            space_to_underbar(n)
                        }
                    })
                    .unwrap_or_default();

                // Format with final star on last subscript
                let demand_formatted = self.format_var_with_final_star(name, subscripts);
                (last, demand_formatted)
            }
        } else {
            // Demand is not a simple variable - format normally, empty subscript
            (String::new(), self.format_expr_inner(demand))
        };

        format!(
            "ALLOCATE({}, {}, {}, {}, {})",
            supply, last_subscript, demand_str, priority, width
        )
    }

    fn format_var_with_final_star(&self, name: &str, subscripts: &[Subscript<'_>]) -> String {
        let formatted_name = self.format_name(name);
        if subscripts.is_empty() {
            return formatted_name;
        }

        let mut subs: Vec<String> = subscripts
            .iter()
            .map(|s| match s {
                Subscript::Element(n, _) | Subscript::BangElement(n, _) => space_to_underbar(n),
            })
            .collect();

        // Append .* to last subscript to indicate "all elements"
        if let Some(last) = subs.last_mut() {
            *last = format!("{}.*", last);
        }

        format!("{}[{}]", formatted_name, subs.join(", "))
    }

    fn format_unary(&self, op: UnaryOp, inner: &Expr<'_>) -> String {
        let inner_str = self.format_expr_inner(inner);
        match op {
            UnaryOp::Positive => format!("+{}", inner_str),
            UnaryOp::Negative => format!("-{}", inner_str),
            UnaryOp::Not => format!(" not {}", inner_str),
        }
    }

    fn format_binary(&self, op: BinaryOp, left: &Expr<'_>, right: &Expr<'_>) -> String {
        let left_str = self.format_expr_inner(left);
        let right_str = self.format_expr_inner(right);

        let op_str = match op {
            BinaryOp::Add => " + ",
            BinaryOp::Sub => " - ",
            BinaryOp::Mul => " * ",
            BinaryOp::Div => " / ",
            BinaryOp::Exp => " ^ ",
            BinaryOp::Lt => " < ",
            BinaryOp::Gt => " > ",
            BinaryOp::Lte => " <= ",
            BinaryOp::Gte => " >= ",
            BinaryOp::Eq => " = ",
            BinaryOp::Neq => " <> ",
            BinaryOp::And => " and ",
            BinaryOp::Or => " or ",
        };

        format!("{}{}{}", left_str, op_str, right_str)
    }

    /// Format a lookup table to XMILE graphical function points.
    #[allow(dead_code)]
    pub fn format_lookup_table(&self, table: &LookupTable) -> (Vec<f64>, Vec<f64>) {
        (table.x_vals.clone(), table.y_vals.clone())
    }
}

/// Format a number using %g-style formatting (matches xmutil's StringFromDouble).
fn format_number(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }

    // Use %g-style formatting
    let abs = value.abs();
    if (1e-4..1e6).contains(&abs) {
        // Use decimal notation
        let s = format!("{}", value);
        // Strip trailing zeros after decimal point
        if s.contains('.') {
            let trimmed = s.trim_end_matches('0').trim_end_matches('.');
            trimmed.to_string()
        } else {
            s
        }
    } else {
        // Use scientific notation
        format!("{:e}", value)
    }
}

/// Replace spaces with underscores in a name.
pub fn space_to_underbar(name: &str) -> String {
    name.replace(' ', "_")
}

/// Replace spaces with underscores, quoting if the name contains periods.
pub fn quoted_space_to_underbar(name: &str) -> String {
    let result = name.replace(' ', "_");
    if result.contains('.') && !result.starts_with('"') {
        format!("\"{}\"", result)
    } else {
        result
    }
}

/// Format a unit expression to a string.
pub fn format_unit_expr(expr: &crate::mdl::ast::UnitExpr<'_>) -> String {
    use crate::mdl::ast::UnitExpr;
    match expr {
        UnitExpr::Unit(name, _) => space_to_underbar(name),
        UnitExpr::Mul(left, right, _) => {
            format!("{} * {}", format_unit_expr(left), format_unit_expr(right))
        }
        UnitExpr::Div(left, right, _) => {
            format!("{} / {}", format_unit_expr(left), format_unit_expr(right))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mdl::ast::Loc;
    use std::borrow::Cow;

    fn loc() -> Loc {
        Loc::default()
    }

    #[test]
    fn test_format_number_zero() {
        assert_eq!(format_number(0.0), "0");
    }

    #[test]
    fn test_format_number_integer() {
        assert_eq!(format_number(42.0), "42");
    }

    #[test]
    fn test_format_number_decimal() {
        assert_eq!(format_number(3.14258), "3.14258");
    }

    #[test]
    fn test_format_number_trailing_zeros() {
        assert_eq!(format_number(1.50), "1.5");
    }

    #[test]
    fn test_space_to_underbar() {
        assert_eq!(space_to_underbar("my variable"), "my_variable");
        assert_eq!(space_to_underbar("no_spaces"), "no_spaces");
    }

    #[test]
    fn test_quoted_space_to_underbar() {
        assert_eq!(quoted_space_to_underbar("simple"), "simple");
        assert_eq!(quoted_space_to_underbar("my variable"), "my_variable");
        assert_eq!(
            quoted_space_to_underbar("var.with.dots"),
            "\"var.with.dots\""
        );
    }

    #[test]
    fn test_format_const() {
        let formatter = XmileFormatter::new();
        let expr = Expr::Const(42.5, loc());
        assert_eq!(formatter.format_expr(&expr), "42.5");
    }

    #[test]
    fn test_format_var_simple() {
        let formatter = XmileFormatter::new();
        let expr = Expr::Var(Cow::Borrowed("my variable"), vec![], loc());
        assert_eq!(formatter.format_expr(&expr), "my_variable");
    }

    #[test]
    fn test_format_var_subscripted() {
        let formatter = XmileFormatter::new();
        let expr = Expr::Var(
            Cow::Borrowed("arr"),
            vec![
                Subscript::Element(Cow::Borrowed("DimA"), loc()),
                Subscript::Element(Cow::Borrowed("DimB"), loc()),
            ],
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "arr[DimA, DimB]");
    }

    #[test]
    fn test_format_binary_add() {
        let formatter = XmileFormatter::new();
        let expr = Expr::Op2(
            BinaryOp::Add,
            Box::new(Expr::Var(Cow::Borrowed("a"), vec![], loc())),
            Box::new(Expr::Var(Cow::Borrowed("b"), vec![], loc())),
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "a + b");
    }

    #[test]
    fn test_format_unary_negative() {
        let formatter = XmileFormatter::new();
        let expr = Expr::Op1(
            UnaryOp::Negative,
            Box::new(Expr::Var(Cow::Borrowed("x"), vec![], loc())),
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "-x");
    }

    #[test]
    fn test_format_if_then_else() {
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("IF THEN ELSE"),
            vec![],
            vec![
                Expr::Var(Cow::Borrowed("cond"), vec![], loc()),
                Expr::Const(1.0, loc()),
                Expr::Const(0.0, loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "( IF cond THEN 1 ELSE 0 )");
    }

    #[test]
    fn test_format_log_one_arg() {
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("LOG"),
            vec![],
            vec![Expr::Var(Cow::Borrowed("x"), vec![], loc())],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "LOG10(x)");
    }

    #[test]
    fn test_format_log_two_args() {
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("LOG"),
            vec![],
            vec![
                Expr::Var(Cow::Borrowed("x"), vec![], loc()),
                Expr::Const(2.0, loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "(LN(x) / LN(2))");
    }

    #[test]
    fn test_format_lookup_invocation() {
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("my_table"),
            vec![],
            vec![Expr::Var(Cow::Borrowed("input"), vec![], loc())],
            CallKind::Symbol,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "LOOKUP(my_table, input)");
    }

    #[test]
    fn test_format_time_name() {
        let formatter = XmileFormatter::new();
        let expr = Expr::Var(Cow::Borrowed("Time"), vec![], loc());
        assert_eq!(formatter.format_expr(&expr), "TIME");
    }

    #[test]
    fn test_format_initial_time_name() {
        let formatter = XmileFormatter::new();
        let expr = Expr::Var(Cow::Borrowed("INITIAL TIME"), vec![], loc());
        assert_eq!(formatter.format_expr(&expr), "STARTTIME");
    }

    #[test]
    fn test_format_paren() {
        let formatter = XmileFormatter::new();
        let expr = Expr::Paren(
            Box::new(Expr::Op2(
                BinaryOp::Add,
                Box::new(Expr::Var(Cow::Borrowed("a"), vec![], loc())),
                Box::new(Expr::Var(Cow::Borrowed("b"), vec![], loc())),
                loc(),
            )),
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "(a + b)");
    }

    #[test]
    fn test_format_logical_operators() {
        let formatter = XmileFormatter::new();
        let expr = Expr::Op2(
            BinaryOp::And,
            Box::new(Expr::Var(Cow::Borrowed("a"), vec![], loc())),
            Box::new(Expr::Var(Cow::Borrowed("b"), vec![], loc())),
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "a and b");
    }

    #[test]
    fn test_format_quantum() {
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("QUANTUM"),
            vec![],
            vec![
                Expr::Var(Cow::Borrowed("x"), vec![], loc()),
                Expr::Const(0.5, loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "(0.5)*INT((x)/(0.5))");
    }

    #[test]
    fn test_format_random_0_1() {
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("RANDOM 0 1"),
            vec![],
            vec![],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "UNIFORM(0, 1)");
    }

    #[test]
    fn test_format_zidz() {
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("ZIDZ"),
            vec![],
            vec![
                Expr::Var(Cow::Borrowed("a"), vec![], loc()),
                Expr::Var(Cow::Borrowed("b"), vec![], loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "SAFEDIV(a, b)");
    }

    #[test]
    fn test_format_xidz() {
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("XIDZ"),
            vec![],
            vec![
                Expr::Var(Cow::Borrowed("a"), vec![], loc()),
                Expr::Var(Cow::Borrowed("b"), vec![], loc()),
                Expr::Const(1.0, loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "SAFEDIV(a, b, 1)");
    }

    #[test]
    fn test_format_pulse() {
        // PULSE(start, width) -> IF TIME >= (start) AND TIME < ((start) + MAX(DT,width)) THEN 1 ELSE 0
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("PULSE"),
            vec![],
            vec![Expr::Const(5.0, loc()), Expr::Const(2.0, loc())],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(
            formatter.format_expr(&expr),
            "( IF TIME >= (5) AND TIME < ((5) + MAX(DT,2)) THEN 1 ELSE 0 )"
        );
    }

    #[test]
    fn test_format_pulse_train() {
        // PULSE TRAIN(start, width, interval, end) ->
        // IF TIME >= (start) AND TIME <= (end) AND (TIME - (start)) MOD (interval) < (width) THEN 1 ELSE 0
        // Note: Unlike PULSE, PULSE TRAIN uses width directly (not MAX(DT, width)) per xmutil
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("PULSE TRAIN"),
            vec![],
            vec![
                Expr::Const(1.0, loc()),
                Expr::Const(0.5, loc()),
                Expr::Const(5.0, loc()),
                Expr::Const(20.0, loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(
            formatter.format_expr(&expr),
            "( IF TIME >= (1) AND TIME <= (20) AND (TIME - (1)) MOD (5) < (0.5) THEN 1 ELSE 0 )"
        );
    }

    #[test]
    fn test_format_time_base() {
        // TIME BASE(t, dt) -> t + (dt) * TIME
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("TIME BASE"),
            vec![],
            vec![
                Expr::Var(Cow::Borrowed("t"), vec![], loc()),
                Expr::Var(Cow::Borrowed("dt"), vec![], loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "t + (dt) * TIME");
    }

    #[test]
    fn test_format_allocate_by_priority() {
        // ALLOCATE BY PRIORITY(demand[region], priority, ignore, width, supply)
        // -> ALLOCATE(supply, region, demand[region.*], priority, width)
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("ALLOCATE BY PRIORITY"),
            vec![],
            vec![
                // demand[region]
                Expr::Var(
                    Cow::Borrowed("demand"),
                    vec![Subscript::Element(Cow::Borrowed("region"), loc())],
                    loc(),
                ),
                // priority
                Expr::Var(Cow::Borrowed("priority"), vec![], loc()),
                // ignore (unused in output)
                Expr::Const(0.0, loc()),
                // width
                Expr::Var(Cow::Borrowed("width"), vec![], loc()),
                // supply
                Expr::Var(Cow::Borrowed("supply"), vec![], loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(
            formatter.format_expr(&expr),
            "ALLOCATE(supply, region, demand[region.*], priority, width)"
        );
    }

    #[test]
    fn test_format_allocate_by_priority_multidim() {
        // ALLOCATE BY PRIORITY(demand[region, product], priority, ignore, width, supply)
        // -> ALLOCATE(supply, product, demand[region, product.*], priority, width)
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("ALLOCATE BY PRIORITY"),
            vec![],
            vec![
                // demand[region, product]
                Expr::Var(
                    Cow::Borrowed("demand"),
                    vec![
                        Subscript::Element(Cow::Borrowed("region"), loc()),
                        Subscript::Element(Cow::Borrowed("product"), loc()),
                    ],
                    loc(),
                ),
                // priority
                Expr::Var(Cow::Borrowed("priority"), vec![], loc()),
                // ignore (unused in output)
                Expr::Const(0.0, loc()),
                // width
                Expr::Var(Cow::Borrowed("width"), vec![], loc()),
                // supply
                Expr::Var(Cow::Borrowed("supply"), vec![], loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(
            formatter.format_expr(&expr),
            "ALLOCATE(supply, product, demand[region, product.*], priority, width)"
        );
    }

    #[test]
    fn test_format_a_function_of() {
        // A FUNCTION OF(x, y) -> NAN (literal, not NAN(x, y))
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("A FUNCTION OF"),
            vec![],
            vec![
                Expr::Var(Cow::Borrowed("x"), vec![], loc()),
                Expr::Var(Cow::Borrowed("y"), vec![], loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "NAN");
    }

    #[test]
    fn test_format_integer() {
        // INTEGER(x) -> INT(x)
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("INTEGER"),
            vec![],
            vec![Expr::Var(Cow::Borrowed("x"), vec![], loc())],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "INT(x)");
    }

    #[test]
    fn test_format_lookup_invert() {
        // LOOKUP INVERT(table, value) -> LOOKUPINV(table, value)
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("LOOKUP INVERT"),
            vec![],
            vec![
                Expr::Var(Cow::Borrowed("my_table"), vec![], loc()),
                Expr::Const(0.5, loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "LOOKUPINV(my_table, 0.5)");
    }

    #[test]
    fn test_format_bang_subscript() {
        // x[dim!] -> x[*] (bang means "iterate over all elements")
        let formatter = XmileFormatter::new();
        let expr = Expr::Var(
            Cow::Borrowed("arr"),
            vec![Subscript::BangElement(Cow::Borrowed("DimA"), loc())],
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "arr[*]");
    }

    #[test]
    fn test_format_mixed_subscripts() {
        // x[DimA, DimB!] -> x[DimA, *]
        let formatter = XmileFormatter::new();
        let expr = Expr::Var(
            Cow::Borrowed("arr"),
            vec![
                Subscript::Element(Cow::Borrowed("DimA"), loc()),
                Subscript::BangElement(Cow::Borrowed("DimB"), loc()),
            ],
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "arr[DimA, *]");
    }

    #[test]
    fn test_format_active_initial() {
        // ACTIVE INITIAL(expr, init) -> INIT(expr, init)
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("ACTIVE INITIAL"),
            vec![],
            vec![
                Expr::Op2(
                    BinaryOp::Mul,
                    Box::new(Expr::Var(Cow::Borrowed("a"), vec![], loc())),
                    Box::new(Expr::Var(Cow::Borrowed("b"), vec![], loc())),
                    loc(),
                ),
                Expr::Const(100.0, loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "INIT(a * b, 100)");
    }

    // M2: Missing XMILE function renames

    #[test]
    fn test_format_vmax_to_max() {
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("VMAX"),
            vec![],
            vec![Expr::Var(Cow::Borrowed("arr"), vec![], loc())],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "MAX(arr)");
    }

    #[test]
    fn test_format_vmin_to_min() {
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("VMIN"),
            vec![],
            vec![Expr::Var(Cow::Borrowed("arr"), vec![], loc())],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "MIN(arr)");
    }

    #[test]
    fn test_format_forecast_to_forcst() {
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("FORECAST"),
            vec![],
            vec![
                Expr::Var(Cow::Borrowed("input"), vec![], loc()),
                Expr::Const(10.0, loc()),
                Expr::Const(5.0, loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "FORCST(input, 10, 5)");
    }

    #[test]
    fn test_format_random_pink_noise_to_normalpink() {
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("RANDOM PINK NOISE"),
            vec![],
            vec![
                Expr::Const(0.0, loc()),
                Expr::Const(1.0, loc()),
                Expr::Const(1.0, loc()),
                Expr::Const(123.0, loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "NORMALPINK(0, 1, 1, 123)");
    }

    #[test]
    fn test_format_reinitial_to_init() {
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("REINITIAL"),
            vec![],
            vec![Expr::Var(Cow::Borrowed("x"), vec![], loc())],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "INIT(x)");
    }

    #[test]
    fn test_format_vector_select_preserves_spaces() {
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("VECTOR SELECT"),
            vec![],
            vec![
                Expr::Var(Cow::Borrowed("sel"), vec![], loc()),
                Expr::Var(Cow::Borrowed("vals"), vec![], loc()),
                Expr::Var(Cow::Borrowed("idx"), vec![], loc()),
                Expr::Const(0.0, loc()),
                Expr::Const(1.0, loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        let result = formatter.format_expr(&expr);
        assert!(
            result.starts_with("VECTOR SELECT("),
            "Should be 'VECTOR SELECT(...)' not 'VECTOR_SELECT(...)': {}",
            result
        );
    }

    #[test]
    fn test_format_vector_elm_map_preserves_spaces() {
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("VECTOR ELM MAP"),
            vec![],
            vec![
                Expr::Var(Cow::Borrowed("vec"), vec![], loc()),
                Expr::Var(Cow::Borrowed("idx"), vec![], loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        let result = formatter.format_expr(&expr);
        assert!(
            result.starts_with("VECTOR ELM MAP("),
            "Should be 'VECTOR ELM MAP(...)': {}",
            result
        );
    }

    #[test]
    fn test_format_vector_sort_order_preserves_spaces() {
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("VECTOR SORT ORDER"),
            vec![],
            vec![
                Expr::Var(Cow::Borrowed("vec"), vec![], loc()),
                Expr::Const(1.0, loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        let result = formatter.format_expr(&expr);
        assert!(
            result.starts_with("VECTOR SORT ORDER("),
            "Should be 'VECTOR SORT ORDER(...)': {}",
            result
        );
    }

    #[test]
    fn test_format_vector_reorder_uses_underscore() {
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("VECTOR REORDER"),
            vec![],
            vec![
                Expr::Var(Cow::Borrowed("vec"), vec![], loc()),
                Expr::Var(Cow::Borrowed("order"), vec![], loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        let result = formatter.format_expr(&expr);
        assert!(
            result.starts_with("VECTOR_REORDER("),
            "Should be 'VECTOR_REORDER(...)' with underscore: {}",
            result
        );
    }

    #[test]
    fn test_format_vector_lookup_preserves_spaces() {
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("VECTOR LOOKUP"),
            vec![],
            vec![
                Expr::Var(Cow::Borrowed("vec"), vec![], loc()),
                Expr::Var(Cow::Borrowed("idx"), vec![], loc()),
                Expr::Const(0.0, loc()),
            ],
            CallKind::Builtin,
            loc(),
        );
        let result = formatter.format_expr(&expr);
        assert!(
            result.starts_with("VECTOR LOOKUP("),
            "Should be 'VECTOR LOOKUP(...)': {}",
            result
        );
    }

    // M3: Bang-subscript formatting

    #[test]
    fn test_bang_subscript_full_dimension_outputs_star() {
        // For full dimensions (not subranges), output just *
        let formatter = XmileFormatter::new();
        let expr = Expr::Var(
            Cow::Borrowed("x"),
            vec![Subscript::BangElement(Cow::Borrowed("DimA"), loc())],
            loc(),
        );
        // DimA is not a subrange (not in subrange_dims set), should output *
        assert_eq!(formatter.format_expr(&expr), "x[*]");
    }

    #[test]
    fn test_bang_subscript_subrange_outputs_name_dot_star() {
        // For subranges (has maps_to), output SubRange.*
        use std::collections::HashSet;
        let mut subranges = HashSet::new();
        subranges.insert("suba".to_string()); // canonical name
        let formatter = XmileFormatter::with_subranges(subranges);

        let expr = Expr::Var(
            Cow::Borrowed("x"),
            vec![Subscript::BangElement(Cow::Borrowed("SubA"), loc())],
            loc(),
        );
        // SubA is a subrange, should output SubA.*
        assert_eq!(formatter.format_expr(&expr), "x[SubA.*]");
    }

    #[test]
    fn test_bang_subscript_mixed_regular_and_subrange() {
        // Mixed subscripts: regular element and bang on subrange
        use std::collections::HashSet;
        let mut subranges = HashSet::new();
        subranges.insert("suba".to_string());
        let formatter = XmileFormatter::with_subranges(subranges);

        let expr = Expr::Var(
            Cow::Borrowed("arr"),
            vec![
                Subscript::Element(Cow::Borrowed("DimA"), loc()),
                Subscript::BangElement(Cow::Borrowed("SubA"), loc()),
            ],
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "arr[DimA, SubA.*]");
    }
}
