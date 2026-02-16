// Copyright 2026 The Simlin Authors. All rights reserved.
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

use std::collections::{HashMap, HashSet};

use crate::mdl::ast::{BinaryOp, CallKind, Expr, LookupTable, Subscript, UnaryOp};
use crate::mdl::builtins::{eq_lower_space, to_lower_space};

/// Context for per-element equation substitution.
/// Maps generic LHS dimension names to specific element names for the
/// current element being expanded. Equivalent to C++ ContextInfo's
/// pLHSElmsGeneric/pLHSElmsSpecific pair.
pub struct ElementContext {
    /// Canonical name of the LHS variable being computed.
    /// Used to detect self-references and emit "self" instead of the variable name.
    pub lhs_var_canonical: String,
    /// dimension canonical name -> specific element (space_to_underbar format)
    /// e.g. {"scenario" -> "deterministic", "upper" -> "layer1"}
    pub substitutions: HashMap<String, String>,
    /// For dimensions NOT directly on the LHS but reachable via subrange
    /// relationships. Maps dim canonical name -> SubrangeMapping.
    pub subrange_mappings: HashMap<String, SubrangeMapping>,
}

/// Mapping information for resolving subrange references positionally.
pub struct SubrangeMapping {
    /// The LHS dimension this subrange maps through
    pub lhs_dim_canonical: String,
    /// Elements of the LHS dimension (in definition order)
    pub lhs_dim_elements: Vec<String>,
    /// Elements of this subrange (in definition order)
    pub own_elements: Vec<String>,
}

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
        self.format_expr_ctx(expr, None)
    }

    /// Format an expression with per-element substitution context.
    pub fn format_expr_with_context(&self, expr: &Expr<'_>, ctx: &ElementContext) -> String {
        self.format_expr_ctx(expr, Some(ctx))
    }

    fn format_expr_ctx(&self, expr: &Expr<'_>, ctx: Option<&ElementContext>) -> String {
        match expr {
            Expr::Const(value, _) => format_number(*value),
            Expr::Var(name, subscripts, _) => self.format_var_ctx(name, subscripts, ctx),
            Expr::App(name, subscripts, args, kind, _) => {
                self.format_call_ctx(name, subscripts, args, *kind, ctx)
            }
            Expr::Op1(op, inner, _) => self.format_unary_ctx(*op, inner, ctx),
            Expr::Op2(op, left, right, _) => self.format_binary_ctx(*op, left, right, ctx),
            Expr::Paren(inner, _) => format!("({})", self.format_expr_ctx(inner, ctx)),
            Expr::Literal(lit, _) => {
                // Literals are already quoted in the AST, output as-is for XMILE
                // But xmutil strips quotes from literals in expression output
                lit.to_string()
            }
            Expr::Na(_) => ":NA:".to_string(),
        }
    }

    fn format_var_ctx(
        &self,
        name: &str,
        subscripts: &[Subscript<'_>],
        ctx: Option<&ElementContext>,
    ) -> String {
        // Detect self-references: if this variable is the LHS variable, emit "self"
        // instead of the variable name, matching xmutil's Variable::OutputComputable behavior.
        let formatted_name = if let Some(ctx) = ctx {
            if !ctx.lhs_var_canonical.is_empty() && eq_lower_space(name, &ctx.lhs_var_canonical) {
                "self".to_string()
            } else {
                self.format_name(name)
            }
        } else {
            self.format_name(name)
        };
        if subscripts.is_empty() {
            formatted_name
        } else {
            let subs: Vec<String> = subscripts
                .iter()
                .map(|s| match s {
                    Subscript::Element(n, _) => {
                        if let Some(ctx) = ctx {
                            let canonical = to_lower_space(n);
                            // Direct substitution: dimension name -> specific element
                            if let Some(specific) = ctx.substitutions.get(&canonical) {
                                return specific.clone();
                            }
                            // Subrange resolution: positional mapping through parent
                            if let Some(mapping) = ctx.subrange_mappings.get(&canonical)
                                && let Some(resolved) = Self::resolve_subrange_element(ctx, mapping)
                            {
                                return resolved;
                            }
                        }
                        space_to_underbar(n)
                    }
                    // Bang subscript `dim!` means "iterate over all elements"
                    // For full dimensions -> `*`
                    // For subranges (have maps_to) -> `Dim.*`
                    // Bang subscripts are never substituted.
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

    /// Resolve a subrange element positionally through a SubrangeMapping.
    /// Given the current context's specific element for the LHS dimension,
    /// find the corresponding element in the subrange by position.
    fn resolve_subrange_element(ctx: &ElementContext, mapping: &SubrangeMapping) -> Option<String> {
        // Get the specific element for the LHS dimension
        let lhs_element = ctx.substitutions.get(&mapping.lhs_dim_canonical)?;
        // Find its position in the LHS dimension's element list
        let pos = mapping
            .lhs_dim_elements
            .iter()
            .position(|e| e == lhs_element)?;
        // Return the corresponding element from the subrange
        mapping.own_elements.get(pos).cloned()
    }

    fn format_name(&self, name: &str) -> String {
        // Handle special TIME-related names without allocating
        if self.use_xmile_time_names {
            if eq_lower_space(name, "time") {
                return "TIME".to_string();
            }
            if eq_lower_space(name, "initial time") {
                return "STARTTIME".to_string();
            }
            if eq_lower_space(name, "final time") {
                return "STOPTIME".to_string();
            }
            if eq_lower_space(name, "time step") {
                return "DT".to_string();
            }
            if eq_lower_space(name, "saveper") {
                return "SAVEPER".to_string();
            }
        }

        // Apply space-to-underbar transformation
        quoted_space_to_underbar(name)
    }

    fn format_call_ctx(
        &self,
        name: &str,
        subscripts: &[Subscript<'_>],
        args: &[Expr<'_>],
        kind: CallKind,
        ctx: Option<&ElementContext>,
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
                        self.format_expr_ctx(&args[0], ctx),
                        self.format_expr_ctx(&args[1], ctx),
                        self.format_expr_ctx(&args[2], ctx)
                    );
                }
            }
            "log" => {
                // LOG in Vensim: 1 arg = LOG10, 2 args = LOG(x, base) = LN(x)/LN(base)
                if args.len() == 1 {
                    return format!("LOG10({})", self.format_expr_ctx(&args[0], ctx));
                } else if args.len() == 2 {
                    return format!(
                        "(LN({}) / LN({}))",
                        self.format_expr_ctx(&args[0], ctx),
                        self.format_expr_ctx(&args[1], ctx)
                    );
                }
            }
            "elmcount" => {
                if !args.is_empty() {
                    return format!("SIZE({})", self.format_expr_ctx(&args[0], ctx));
                }
            }
            "delay n" => {
                // DELAY N(input, dt, init, n) -> DELAYN(input, dt, n, init)
                if args.len() >= 4 {
                    return format!(
                        "DELAYN({}, {}, {}, {})",
                        self.format_expr_ctx(&args[0], ctx),
                        self.format_expr_ctx(&args[1], ctx),
                        self.format_expr_ctx(&args[3], ctx),
                        self.format_expr_ctx(&args[2], ctx)
                    );
                }
            }
            "smooth n" => {
                // SMOOTH N(input, dt, init, n) -> SMTHN(input, dt, n, init)
                if args.len() >= 4 {
                    return format!(
                        "SMTHN({}, {}, {}, {})",
                        self.format_expr_ctx(&args[0], ctx),
                        self.format_expr_ctx(&args[1], ctx),
                        self.format_expr_ctx(&args[3], ctx),
                        self.format_expr_ctx(&args[2], ctx)
                    );
                }
            }
            "random normal" => {
                // RANDOM NORMAL(min, max, mean, sd, seed) -> NORMAL(mean, sd, seed, min, max)
                if args.len() >= 5 {
                    return format!(
                        "NORMAL({}, {}, {}, {}, {})",
                        self.format_expr_ctx(&args[2], ctx),
                        self.format_expr_ctx(&args[3], ctx),
                        self.format_expr_ctx(&args[4], ctx),
                        self.format_expr_ctx(&args[0], ctx),
                        self.format_expr_ctx(&args[1], ctx)
                    );
                }
            }
            "quantum" => {
                // QUANTUM(x, q) -> (q)*INT((x)/(q))
                if args.len() >= 2 {
                    let x = self.format_expr_ctx(&args[0], ctx);
                    let q = self.format_expr_ctx(&args[1], ctx);
                    return format!("({})*INT(({})/({}))", q, x, q);
                }
            }
            "pulse" => {
                // PULSE(start, width) -> IF TIME >= (start) AND TIME < ((start) + MAX(DT,width)) THEN 1 ELSE 0
                if args.len() >= 2 {
                    let start = self.format_expr_ctx(&args[0], ctx);
                    let width = self.format_expr_ctx(&args[1], ctx);
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
                    let start = self.format_expr_ctx(&args[0], ctx);
                    let width = self.format_expr_ctx(&args[1], ctx);
                    let interval = self.format_expr_ctx(&args[2], ctx);
                    let end = self.format_expr_ctx(&args[3], ctx);
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
                        self.format_expr_ctx(&args[0], ctx),
                        self.format_expr_ctx(&args[1], ctx),
                        self.format_expr_ctx(&args[2], ctx)
                    );
                }
            }
            "allocate by priority" => {
                // ALLOCATE BY PRIORITY with reordered args
                return self.format_allocate_by_priority_ctx(args, ctx);
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
                        self.format_expr_ctx(&args[2], ctx),
                        self.format_expr_ctx(&args[5], ctx),
                        self.format_expr_ctx(&args[0], ctx),
                        self.format_expr_ctx(&args[1], ctx),
                        self.format_expr_ctx(&args[4], ctx),
                        self.format_expr_ctx(&args[3], ctx)
                    );
                }
            }
            "time base" => {
                // TIME BASE(t, dt) -> t + (dt) * TIME
                if args.len() >= 2 {
                    let t = self.format_expr_ctx(&args[0], ctx);
                    let dt = self.format_expr_ctx(&args[1], ctx);
                    return format!("{} + ({}) * TIME", t, dt);
                }
            }
            "zidz" => {
                // ZIDZ(a, b) -> SAFEDIV(a, b)
                if args.len() >= 2 {
                    return format!(
                        "SAFEDIV({}, {})",
                        self.format_expr_ctx(&args[0], ctx),
                        self.format_expr_ctx(&args[1], ctx)
                    );
                }
            }
            "xidz" => {
                // XIDZ(a, b, x) -> SAFEDIV(a, b, x)
                if args.len() >= 3 {
                    return format!(
                        "SAFEDIV({}, {}, {})",
                        self.format_expr_ctx(&args[0], ctx),
                        self.format_expr_ctx(&args[1], ctx),
                        self.format_expr_ctx(&args[2], ctx)
                    );
                }
            }
            _ => {}
        }

        // Check for lookup invocation (Symbol call with 1 arg)
        if kind == CallKind::Symbol && args.len() == 1 {
            let table_name = self.format_var_ctx(name, subscripts, ctx);
            return format!(
                "LOOKUP({}, {})",
                table_name,
                self.format_expr_ctx(&args[0], ctx)
            );
        }

        // Default function call formatting
        let func_name = self.format_function_name(&canonical);
        let formatted_args: Vec<String> =
            args.iter().map(|a| self.format_expr_ctx(a, ctx)).collect();

        if subscripts.is_empty() {
            format!("{}({})", func_name, formatted_args.join(", "))
        } else {
            let subs: Vec<String> = subscripts
                .iter()
                .map(|s| self.format_subscript(s, ctx))
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

    fn format_allocate_by_priority_ctx(
        &self,
        args: &[Expr<'_>],
        ctx: Option<&ElementContext>,
    ) -> String {
        // ALLOCATE BY PRIORITY(demand, priority, ignore, width, supply)
        // -> ALLOCATE(supply, last_subscript, demand_with_star, priority, width)
        if args.len() != 5 {
            // Fallback: pass through as-is
            let formatted: Vec<String> =
                args.iter().map(|a| self.format_expr_ctx(a, ctx)).collect();
            return format!("ALLOCATE_BY_PRIORITY({})", formatted.join(", "));
        }

        let supply = self.format_expr_ctx(&args[4], ctx);
        let demand = &args[0];
        let priority = self.format_expr_ctx(&args[1], ctx);
        let width = self.format_expr_ctx(&args[3], ctx);

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
            (String::new(), self.format_expr_ctx(demand, ctx))
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

    /// Format a single subscript with optional context for substitution.
    fn format_subscript(&self, s: &Subscript<'_>, ctx: Option<&ElementContext>) -> String {
        match s {
            Subscript::Element(n, _) => {
                if let Some(ctx) = ctx {
                    let canonical = to_lower_space(n);
                    if let Some(specific) = ctx.substitutions.get(&canonical) {
                        return specific.clone();
                    }
                    if let Some(mapping) = ctx.subrange_mappings.get(&canonical)
                        && let Some(resolved) = Self::resolve_subrange_element(ctx, mapping)
                    {
                        return resolved;
                    }
                }
                space_to_underbar(n)
            }
            Subscript::BangElement(n, _) => {
                let canonical = to_lower_space(n);
                if self.subrange_dims.contains(&canonical) {
                    format!("{}.*", space_to_underbar(n))
                } else {
                    "*".to_string()
                }
            }
        }
    }

    fn format_unary_ctx(
        &self,
        op: UnaryOp,
        inner: &Expr<'_>,
        ctx: Option<&ElementContext>,
    ) -> String {
        let inner_str = self.format_expr_ctx(inner, ctx);
        match op {
            UnaryOp::Positive => format!("+{}", inner_str),
            UnaryOp::Negative => format!("-{}", inner_str),
            UnaryOp::Not => format!(" not {}", inner_str),
        }
    }

    fn format_binary_ctx(
        &self,
        op: BinaryOp,
        left: &Expr<'_>,
        right: &Expr<'_>,
        ctx: Option<&ElementContext>,
    ) -> String {
        let left_str = self.format_expr_ctx(left, ctx);
        let right_str = self.format_expr_ctx(right, ctx);

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

/// Format a number to match C++ std::to_chars shortest round-trip representation.
///
/// C++ std::to_chars picks whichever of decimal or scientific notation is
/// shorter. When using scientific notation, it formats the exponent as
/// `e[+-]dd` (explicit sign, at least 2 digits). Rust's `format!("{}", f64)`
/// already gives the shortest decimal representation, so we compare that
/// with our scientific notation format and pick the shorter one.
pub fn format_number(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }

    // Rust's Display gives shortest round-trip decimal representation
    let decimal = format!("{}", value);

    // Build scientific notation in C++ to_chars style: e[+-]dd (min 2-digit exponent)
    let scientific = format_scientific(value);

    // Pick the shorter representation (C++ to_chars behavior)
    if scientific.len() < decimal.len() {
        scientific
    } else {
        decimal
    }
}

/// Format a value in scientific notation matching C++ to_chars exponent style.
/// Exponent is formatted as `e[+-]dd` with explicit sign and minimum 2 digits.
fn format_scientific(value: f64) -> String {
    // Use Rust's {:e} to get exact mantissa + exponent, then reformat the exponent
    let s = format!("{:e}", value);
    // s is like "3.1536e-15" or "1e6" or "-8e-5"
    if let Some(e_pos) = s.find('e') {
        let mantissa = &s[..e_pos];
        let exp_str = &s[e_pos + 1..];
        let exp: i32 = exp_str.parse().unwrap_or(0);

        let exp_sign = if exp >= 0 { '+' } else { '-' };
        let exp_abs = exp.unsigned_abs();
        if exp_abs >= 100 {
            format!("{}e{}{}", mantissa, exp_sign, exp_abs)
        } else {
            format!("{}e{}{:02}", mantissa, exp_sign, exp_abs)
        }
    } else {
        s
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

/// Simplified unit representation with separate numerator/denominator.
/// This mirrors xmutil's UnitExpression with vNumerator/vDenominator vectors.
struct SimplifiedUnit {
    numerator: Vec<String>,
    denominator: Vec<String>,
}

impl SimplifiedUnit {
    fn new() -> Self {
        SimplifiedUnit {
            numerator: Vec::new(),
            denominator: Vec::new(),
        }
    }

    /// Simplify by canceling matching terms between numerator and denominator.
    fn simplify(&mut self) {
        let mut i = 0;
        while i < self.numerator.len() {
            if let Some(j) = self
                .denominator
                .iter()
                .position(|d| d == &self.numerator[i])
            {
                self.numerator.remove(i);
                self.denominator.remove(j);
                // Don't increment i - next element shifted into position
            } else {
                i += 1;
            }
        }
    }

    /// Format in xmutil-compatible canonical form.
    fn format(&self) -> String {
        let num = if self.numerator.is_empty() {
            "1".to_string()
        } else {
            self.numerator.join("*")
        };

        if self.denominator.is_empty() {
            num
        } else if self.denominator.len() == 1 {
            format!("{}/{}", num, self.denominator[0])
        } else {
            // Multiple denominators: use "/(A*B)" form like xmutil
            format!("{}/({})", num, self.denominator.join("*"))
        }
    }
}

/// Flatten a UnitExpr tree into a SimplifiedUnit with numerator/denominator lists.
fn flatten_unit_expr(
    expr: &crate::mdl::ast::UnitExpr<'_>,
    is_denominator: bool,
    result: &mut SimplifiedUnit,
) {
    use crate::mdl::ast::UnitExpr;
    match expr {
        UnitExpr::Unit(name, _) => {
            let name = space_to_underbar(name);
            if is_denominator {
                result.denominator.push(name);
            } else {
                result.numerator.push(name);
            }
        }
        UnitExpr::Mul(left, right, _) => {
            // Both sides go to same list (numerator or denominator)
            flatten_unit_expr(left, is_denominator, result);
            flatten_unit_expr(right, is_denominator, result);
        }
        UnitExpr::Div(left, right, _) => {
            // Left goes to current list, right goes to opposite
            flatten_unit_expr(left, is_denominator, result);
            flatten_unit_expr(right, !is_denominator, result);
        }
    }
}

/// Format a unit expression to a simplified, canonical string.
///
/// This mirrors xmutil's UnitExpression::GetEquationString() behavior:
/// - Flattens to numerator/denominator lists
/// - Cancels matching terms (e.g., "A/A" -> "1")
/// - Outputs canonical form with "/(X*Y)" for compound denominators
pub fn format_unit_expr(expr: &crate::mdl::ast::UnitExpr<'_>) -> String {
    let mut simplified = SimplifiedUnit::new();
    flatten_unit_expr(expr, false, &mut simplified);
    simplified.simplify();
    simplified.format()
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
    fn test_format_number_scientific_large() {
        // C++ to_chars uses scientific with e+dd format for large values
        assert_eq!(format_number(1e6), "1e+06");
        assert_eq!(format_number(1e9), "1e+09");
        assert_eq!(format_number(5.1e14), "5.1e+14");
    }

    #[test]
    fn test_format_number_scientific_small() {
        // C++ to_chars uses scientific with e-dd format for small values
        assert_eq!(format_number(8e-5), "8e-05");
        assert_eq!(format_number(2.01e-5), "2.01e-05");
        assert_eq!(format_number(1.3264e-6), "1.3264e-06");
        assert_eq!(format_number(5.68e-9), "5.68e-09");
        assert_eq!(format_number(3.1536e-15), "3.1536e-15");
    }

    #[test]
    fn test_format_number_boundary_decimal_vs_scientific() {
        // Values that are shorter in decimal remain decimal
        assert_eq!(format_number(100.0), "100");
        assert_eq!(format_number(0.001), "0.001");
        // 0.0001 is 6 chars, 1e-04 is 5 chars: scientific wins
        assert_eq!(format_number(0.0001), "1e-04");
        // Negative values
        assert_eq!(format_number(-42.0), "-42");
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

    // Unit expression simplification tests

    #[test]
    fn test_format_unit_expr_simple() {
        use crate::mdl::ast::UnitExpr;
        let unit = UnitExpr::Unit(Cow::Borrowed("Year"), loc());
        assert_eq!(format_unit_expr(&unit), "Year");
    }

    #[test]
    fn test_format_unit_expr_multiplication() {
        use crate::mdl::ast::UnitExpr;
        // A * B -> "A*B"
        let unit = UnitExpr::Mul(
            Box::new(UnitExpr::Unit(Cow::Borrowed("A"), loc())),
            Box::new(UnitExpr::Unit(Cow::Borrowed("B"), loc())),
            loc(),
        );
        assert_eq!(format_unit_expr(&unit), "A*B");
    }

    #[test]
    fn test_format_unit_expr_division() {
        use crate::mdl::ast::UnitExpr;
        // A / B -> "A/B"
        let unit = UnitExpr::Div(
            Box::new(UnitExpr::Unit(Cow::Borrowed("A"), loc())),
            Box::new(UnitExpr::Unit(Cow::Borrowed("B"), loc())),
            loc(),
        );
        assert_eq!(format_unit_expr(&unit), "A/B");
    }

    #[test]
    fn test_format_unit_expr_compound_denominator() {
        use crate::mdl::ast::UnitExpr;
        // A / (B * C) -> "A/(B*C)"
        let unit = UnitExpr::Div(
            Box::new(UnitExpr::Unit(Cow::Borrowed("A"), loc())),
            Box::new(UnitExpr::Mul(
                Box::new(UnitExpr::Unit(Cow::Borrowed("B"), loc())),
                Box::new(UnitExpr::Unit(Cow::Borrowed("C"), loc())),
                loc(),
            )),
            loc(),
        );
        assert_eq!(format_unit_expr(&unit), "A/(B*C)");
    }

    #[test]
    fn test_format_unit_expr_simplifies_matching_terms() {
        use crate::mdl::ast::UnitExpr;
        // A * B / B -> "A" (B cancels)
        let unit = UnitExpr::Div(
            Box::new(UnitExpr::Mul(
                Box::new(UnitExpr::Unit(Cow::Borrowed("A"), loc())),
                Box::new(UnitExpr::Unit(Cow::Borrowed("B"), loc())),
                loc(),
            )),
            Box::new(UnitExpr::Unit(Cow::Borrowed("B"), loc())),
            loc(),
        );
        assert_eq!(format_unit_expr(&unit), "A");
    }

    #[test]
    fn test_format_unit_expr_full_cancellation() {
        use crate::mdl::ast::UnitExpr;
        // A / A -> "1"
        let unit = UnitExpr::Div(
            Box::new(UnitExpr::Unit(Cow::Borrowed("A"), loc())),
            Box::new(UnitExpr::Unit(Cow::Borrowed("A"), loc())),
            loc(),
        );
        assert_eq!(format_unit_expr(&unit), "1");
    }

    #[test]
    fn test_format_unit_expr_space_to_underbar() {
        use crate::mdl::ast::UnitExpr;
        // "My Unit" -> "My_Unit"
        let unit = UnitExpr::Unit(Cow::Borrowed("My Unit"), loc());
        assert_eq!(format_unit_expr(&unit), "My_Unit");
    }

    #[test]
    fn test_format_unit_expr_chained_division() {
        use crate::mdl::ast::UnitExpr;
        // A / B / C -> "A/(B*C)"
        // In the AST, this is: (A / B) / C
        let unit = UnitExpr::Div(
            Box::new(UnitExpr::Div(
                Box::new(UnitExpr::Unit(Cow::Borrowed("A"), loc())),
                Box::new(UnitExpr::Unit(Cow::Borrowed("B"), loc())),
                loc(),
            )),
            Box::new(UnitExpr::Unit(Cow::Borrowed("C"), loc())),
            loc(),
        );
        assert_eq!(format_unit_expr(&unit), "A/(B*C)");
    }

    #[test]
    fn test_format_unit_expr_complex_simplification() {
        use crate::mdl::ast::UnitExpr;
        // (A * B * C) / (B * C) -> "A" (B and C cancel)
        let unit = UnitExpr::Div(
            Box::new(UnitExpr::Mul(
                Box::new(UnitExpr::Mul(
                    Box::new(UnitExpr::Unit(Cow::Borrowed("A"), loc())),
                    Box::new(UnitExpr::Unit(Cow::Borrowed("B"), loc())),
                    loc(),
                )),
                Box::new(UnitExpr::Unit(Cow::Borrowed("C"), loc())),
                loc(),
            )),
            Box::new(UnitExpr::Mul(
                Box::new(UnitExpr::Unit(Cow::Borrowed("B"), loc())),
                Box::new(UnitExpr::Unit(Cow::Borrowed("C"), loc())),
                loc(),
            )),
            loc(),
        );
        assert_eq!(format_unit_expr(&unit), "A");
    }

    #[test]
    fn test_format_unit_expr_numerator_only() {
        use crate::mdl::ast::UnitExpr;
        // A * B (no denominator) -> "A*B"
        let unit = UnitExpr::Mul(
            Box::new(UnitExpr::Unit(Cow::Borrowed("A"), loc())),
            Box::new(UnitExpr::Unit(Cow::Borrowed("B"), loc())),
            loc(),
        );
        assert_eq!(format_unit_expr(&unit), "A*B");
    }

    #[test]
    fn test_format_lookup_with_element_subscript() {
        // LOOKUP(foo[cop], input) should preserve the subscript on the table name
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("my_table"),
            vec![Subscript::Element(Cow::Borrowed("cop"), loc())],
            vec![Expr::Var(Cow::Borrowed("input"), vec![], loc())],
            CallKind::Symbol,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "LOOKUP(my_table[cop], input)");
    }

    #[test]
    fn test_format_lookup_with_bang_subscript() {
        // LOOKUP(foo[*], input) should preserve the bang subscript on the table name
        let formatter = XmileFormatter::new();
        let expr = Expr::App(
            Cow::Borrowed("my_table"),
            vec![Subscript::BangElement(Cow::Borrowed("DimA"), loc())],
            vec![Expr::Var(Cow::Borrowed("input"), vec![], loc())],
            CallKind::Symbol,
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "LOOKUP(my_table[*], input)");
    }

    #[test]
    fn test_format_lookup_without_subscript() {
        // LOOKUP(foo, input) without subscript should remain unchanged
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
    fn test_bang_subscript_on_subrange_produces_qualified_wildcard() {
        // BangElement on a subrange should produce "subrange_name.*" not "*"
        let subranges: HashSet<String> =
            HashSet::from(["cop developed".to_string(), "cop developing a".to_string()]);
        let formatter = XmileFormatter::with_subranges(subranges);

        let expr = Expr::Var(
            Cow::Borrowed("co2_ff_emissions"),
            vec![Subscript::BangElement(
                Cow::Borrowed("COP Developed"),
                loc(),
            )],
            loc(),
        );
        assert_eq!(
            formatter.format_expr(&expr),
            "co2_ff_emissions[COP_Developed.*]"
        );
    }

    #[test]
    fn test_bang_subscript_on_full_dimension_produces_bare_wildcard() {
        // BangElement on a full dimension (not a subrange) should produce "*"
        let subranges: HashSet<String> = HashSet::from(["cop developed".to_string()]);
        let formatter = XmileFormatter::with_subranges(subranges);

        let expr = Expr::Var(
            Cow::Borrowed("co2_ff_emissions"),
            vec![Subscript::BangElement(Cow::Borrowed("COP"), loc())],
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "co2_ff_emissions[*]");
    }

    #[test]
    fn test_context_substitution_1d() {
        // y[DimA] with context {dima -> a1} should produce y[a1]
        let formatter = XmileFormatter::new();
        let ctx = ElementContext {
            lhs_var_canonical: String::new(),
            substitutions: HashMap::from([("dima".to_string(), "a1".to_string())]),
            subrange_mappings: HashMap::new(),
        };

        let expr = Expr::Var(
            Cow::Borrowed("y"),
            vec![Subscript::Element(Cow::Borrowed("DimA"), loc())],
            loc(),
        );
        assert_eq!(formatter.format_expr_with_context(&expr, &ctx), "y[a1]");
    }

    #[test]
    fn test_context_substitution_2d() {
        // y[DimA, DimB] with context {dima -> a1, dimb -> b2} -> y[a1, b2]
        let formatter = XmileFormatter::new();
        let ctx = ElementContext {
            lhs_var_canonical: String::new(),
            substitutions: HashMap::from([
                ("dima".to_string(), "a1".to_string()),
                ("dimb".to_string(), "b2".to_string()),
            ]),
            subrange_mappings: HashMap::new(),
        };

        let expr = Expr::Var(
            Cow::Borrowed("y"),
            vec![
                Subscript::Element(Cow::Borrowed("DimA"), loc()),
                Subscript::Element(Cow::Borrowed("DimB"), loc()),
            ],
            loc(),
        );
        assert_eq!(formatter.format_expr_with_context(&expr, &ctx), "y[a1, b2]");
    }

    #[test]
    fn test_context_no_false_substitution() {
        // y[a1] with context {dima -> a1}: a1 is an element not a dimension,
        // so it shouldn't be in substitutions and should stay as y[a1]
        let formatter = XmileFormatter::new();
        let ctx = ElementContext {
            lhs_var_canonical: String::new(),
            substitutions: HashMap::from([("dima".to_string(), "a1".to_string())]),
            subrange_mappings: HashMap::new(),
        };

        let expr = Expr::Var(
            Cow::Borrowed("y"),
            vec![Subscript::Element(Cow::Borrowed("a1"), loc())],
            loc(),
        );
        assert_eq!(formatter.format_expr_with_context(&expr, &ctx), "y[a1]");
    }

    #[test]
    fn test_context_nested_expression() {
        // IF x[DimA] > 0 THEN y[DimA] ELSE z[DimA] with context {dima -> a1}
        let formatter = XmileFormatter::new();
        let ctx = ElementContext {
            lhs_var_canonical: String::new(),
            substitutions: HashMap::from([("dima".to_string(), "a1".to_string())]),
            subrange_mappings: HashMap::new(),
        };

        let expr = Expr::App(
            Cow::Borrowed("IF THEN ELSE"),
            vec![],
            vec![
                // x[DimA] > 0
                Expr::Op2(
                    BinaryOp::Gt,
                    Box::new(Expr::Var(
                        Cow::Borrowed("x"),
                        vec![Subscript::Element(Cow::Borrowed("DimA"), loc())],
                        loc(),
                    )),
                    Box::new(Expr::Const(0.0, loc())),
                    loc(),
                ),
                // y[DimA]
                Expr::Var(
                    Cow::Borrowed("y"),
                    vec![Subscript::Element(Cow::Borrowed("DimA"), loc())],
                    loc(),
                ),
                // z[DimA]
                Expr::Var(
                    Cow::Borrowed("z"),
                    vec![Subscript::Element(Cow::Borrowed("DimA"), loc())],
                    loc(),
                ),
            ],
            CallKind::Builtin,
            loc(),
        );
        assert_eq!(
            formatter.format_expr_with_context(&expr, &ctx),
            "( IF x[a1] > 0 THEN y[a1] ELSE z[a1] )"
        );
    }

    #[test]
    fn test_no_context_unchanged() {
        // Calling format_expr (without context) should work identically
        let formatter = XmileFormatter::new();
        let expr = Expr::Var(
            Cow::Borrowed("y"),
            vec![Subscript::Element(Cow::Borrowed("DimA"), loc())],
            loc(),
        );
        assert_eq!(formatter.format_expr(&expr), "y[DimA]");
    }

    #[test]
    fn test_context_subrange_resolution() {
        // x[lower] with context {upper -> layer1} and subrange mapping
        // for "lower" -> positional resolution through "upper"
        let formatter = XmileFormatter::new();
        let ctx = ElementContext {
            lhs_var_canonical: String::new(),
            substitutions: HashMap::from([("upper".to_string(), "layer1".to_string())]),
            subrange_mappings: HashMap::from([(
                "lower".to_string(),
                SubrangeMapping {
                    lhs_dim_canonical: "upper".to_string(),
                    lhs_dim_elements: vec![
                        "layer1".to_string(),
                        "layer2".to_string(),
                        "layer3".to_string(),
                    ],
                    own_elements: vec![
                        "layer2".to_string(),
                        "layer3".to_string(),
                        "layer4".to_string(),
                    ],
                },
            )]),
        };

        let expr = Expr::Var(
            Cow::Borrowed("y"),
            vec![Subscript::Element(Cow::Borrowed("lower"), loc())],
            loc(),
        );
        // layer1 is at position 0 in upper, so lower[0] = layer2
        assert_eq!(formatter.format_expr_with_context(&expr, &ctx), "y[layer2]");
    }

    #[test]
    fn test_format_self_reference() {
        // When the variable references itself, emit "self" instead of the variable name.
        // This matches xmutil's Variable::OutputComputable behavior (Variable.cpp:326-332).
        let formatter = XmileFormatter::new();
        let ctx = ElementContext {
            lhs_var_canonical: "depth at bottom".to_string(),
            substitutions: HashMap::new(),
            subrange_mappings: HashMap::new(),
        };

        let expr = Expr::Var(
            Cow::Borrowed("Depth at Bottom"),
            vec![Subscript::Element(Cow::Borrowed("layer1"), loc())],
            loc(),
        );
        assert_eq!(
            formatter.format_expr_with_context(&expr, &ctx),
            "self[layer1]"
        );
    }

    #[test]
    fn test_format_non_self_reference_unchanged() {
        // A reference to a different variable should NOT be replaced with "self".
        let formatter = XmileFormatter::new();
        let ctx = ElementContext {
            lhs_var_canonical: "depth at bottom".to_string(),
            substitutions: HashMap::new(),
            subrange_mappings: HashMap::new(),
        };

        let expr = Expr::Var(
            Cow::Borrowed("Layer Depth"),
            vec![Subscript::Element(Cow::Borrowed("layer2"), loc())],
            loc(),
        );
        assert_eq!(
            formatter.format_expr_with_context(&expr, &ctx),
            "Layer_Depth[layer2]"
        );
    }
}
