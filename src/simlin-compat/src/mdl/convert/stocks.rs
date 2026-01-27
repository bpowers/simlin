// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Stock and flow linking methods for MDL to datamodel conversion.

use std::collections::HashMap;

use simlin_core::datamodel::{Equation, GraphicalFunction};

use super::ConversionContext;
use super::helpers::{
    canonical_name, cartesian_product, equation_is_stock, extract_constant_value,
    extract_first_units, get_lhs,
};
use super::types::{SyntheticFlow, VariableType};
use crate::mdl::ast::{BinaryOp, CallKind, Equation as MdlEquation, Expr, FullEquation, Subscript};
use crate::mdl::builtins::to_lower_space;

impl<'input> ConversionContext<'input> {
    /// Pass 3: Mark variable types based on equation content.
    pub(super) fn mark_variable_types(&mut self) {
        // Identify control variables and extract their values
        let control_vars = [
            ("initial time", "STARTTIME"),
            ("final time", "STOPTIME"),
            ("time step", "DT"),
            ("saveper", "SAVEPER"),
        ];

        // First pass: extract values from control vars (read-only)
        for (name, _alt_name) in &control_vars {
            if let Some(info) = self.symbols.get(*name)
                && let Some(eq) = self.select_equation(&info.equations)
                && let Some(value) = extract_constant_value(&eq.equation)
            {
                match *name {
                    "initial time" => self.sim_specs.start = Some(value),
                    "final time" => self.sim_specs.stop = Some(value),
                    "time step" => self.sim_specs.dt = Some(value),
                    "saveper" => self.sim_specs.save_step = Some(value),
                    _ => {}
                }
            }
        }

        // Extract time_units using xmutil's priority chain:
        // TIME STEP > FINAL TIME > INITIAL TIME > "Months"
        let time_units = self
            .symbols
            .get("time step")
            .and_then(|info| extract_first_units(&info.equations))
            .or_else(|| {
                self.symbols
                    .get("final time")
                    .and_then(|info| extract_first_units(&info.equations))
            })
            .or_else(|| {
                self.symbols
                    .get("initial time")
                    .and_then(|info| extract_first_units(&info.equations))
            })
            .unwrap_or_else(|| "Months".to_string());
        self.sim_specs.time_units = Some(time_units);

        // Second pass: mark control vars as unwanted (mutably)
        for (name, alt_name) in control_vars {
            if let Some(info) = self.symbols.get_mut(name) {
                info.unwanted = true;
                info.alternate_name = Some(alt_name.to_string());
            }
        }

        // Mark stocks (have top-level INTEG calls)
        let stock_names: Vec<String> = self
            .symbols
            .iter()
            .filter(|(_, info)| {
                info.equations
                    .iter()
                    .any(|eq| equation_is_stock(&eq.equation))
            })
            .map(|(name, _)| name.clone())
            .collect();

        for name in stock_names {
            if let Some(info) = self.symbols.get_mut(&name) {
                info.var_type = VariableType::Stock;
            }
        }
    }

    /// Pass 4: Scan for LOOKUP EXTRAPOLATE / TABXL calls and flag referenced lookups.
    pub(super) fn scan_for_extrapolate_lookups(&mut self) {
        // Collect all expressions first to avoid borrow conflict
        let equations: Vec<_> = self
            .symbols
            .values()
            .flat_map(|info| info.equations.iter())
            .filter_map(|eq| {
                if let MdlEquation::Regular(_, expr) = &eq.equation {
                    Some(expr.clone())
                } else {
                    None
                }
            })
            .collect();

        for expr in &equations {
            self.scan_expr_for_extrapolate(expr);
        }
    }

    fn scan_expr_for_extrapolate(&mut self, expr: &Expr<'_>) {
        match expr {
            Expr::App(name, _, args, CallKind::Builtin, _) => {
                let canonical = to_lower_space(name);
                // Only TABXL marks the lookup table as extrapolating.
                // LOOKUP EXTRAPOLATE performs extrapolation at call time without
                // permanently marking the table (matching xmutil behavior).
                if canonical == "tabxl" && !args.is_empty() {
                    // First arg must be a simple variable reference to a lookup
                    if let Expr::Var(lookup_name, _, _) = &args[0] {
                        self.extrapolate_lookups.insert(canonical_name(lookup_name));
                    }
                    // If first arg is not a simple var (e.g., expression), ignore
                }
                // Recurse into args
                for arg in args {
                    self.scan_expr_for_extrapolate(arg);
                }
            }
            Expr::Op2(_, left, right, _) => {
                self.scan_expr_for_extrapolate(left);
                self.scan_expr_for_extrapolate(right);
            }
            Expr::Op1(_, inner, _) | Expr::Paren(inner, _) => {
                self.scan_expr_for_extrapolate(inner);
            }
            _ => {}
        }
    }

    /// Pass 5: Link stocks and flows by analyzing INTEG rate expressions.
    ///
    /// For each stock:
    /// 1. Collect flow lists from ALL equations (not just the first)
    /// 2. If all flow lists match, use those flows
    /// 3. If any flow list differs or is invalid, synthesize a net flow
    ///
    /// This matches xmutil's MarkStockFlows algorithm (Variable.cpp:210-276).
    pub(super) fn link_stocks_and_flows(&mut self) {
        // Collect stock names first to avoid borrowing issues
        let stock_names: Vec<String> = self
            .symbols
            .iter()
            .filter(|(_, info)| info.var_type == VariableType::Stock)
            .map(|(name, _)| name.clone())
            .collect();

        // A flow list extracted from a single equation
        struct FlowList {
            inflows: Vec<String>,
            outflows: Vec<String>,
            valid: bool,
        }

        impl FlowList {
            fn matches(&self, other: &FlowList) -> bool {
                if !self.valid || !other.valid {
                    return false;
                }
                if self.inflows.len() != other.inflows.len()
                    || self.outflows.len() != other.outflows.len()
                {
                    return false;
                }
                // Check same flows (order doesn't matter per xmutil)
                for inf in &self.inflows {
                    if !other.inflows.contains(inf) {
                        return false;
                    }
                }
                for outf in &self.outflows {
                    if !other.outflows.contains(outf) {
                        return false;
                    }
                }
                true
            }
        }

        // Information about each stock's flows and any synthetic flows to create
        struct StockFlowInfo {
            stock_name: String,
            inflows: Vec<String>,
            outflows: Vec<String>,
            synthetic_flow: Option<(String, Equation)>, // (name, equation)
        }

        let mut flow_info: Vec<StockFlowInfo> = Vec::new();

        for stock_name in &stock_names {
            let info = match self.symbols.get(stock_name) {
                Some(info) => info,
                None => continue,
            };

            // Collect flow lists from ALL valid stock equations (skip EmptyRhs/AFO)
            let stock_equations: Vec<&FullEquation<'_>> = info
                .equations
                .iter()
                .filter(|eq| !self.is_empty_rhs(&eq.equation))
                .filter(|eq| !self.is_afo_expr_in_eq(&eq.equation))
                .filter(|eq| equation_is_stock(&eq.equation))
                .collect();

            if stock_equations.is_empty() {
                continue;
            }

            // Extract flow lists from each equation
            let flow_lists: Vec<FlowList> = stock_equations
                .iter()
                .map(|eq| {
                    if let Some((inflows, outflows)) =
                        self.extract_flows_from_equation(&eq.equation)
                    {
                        FlowList {
                            inflows,
                            outflows,
                            valid: true,
                        }
                    } else {
                        // Flow decomposition failed
                        FlowList {
                            inflows: vec![],
                            outflows: vec![],
                            valid: false,
                        }
                    }
                })
                .collect();

            // Check if all flow lists match the first one
            let all_match = flow_lists
                .iter()
                .skip(1)
                .all(|fl| flow_lists[0].matches(fl));

            if all_match && flow_lists[0].valid {
                // All equations decompose to the same flows - use them
                flow_info.push(StockFlowInfo {
                    stock_name: stock_name.clone(),
                    inflows: flow_lists[0].inflows.clone(),
                    outflows: flow_lists[0].outflows.clone(),
                    synthetic_flow: None,
                });
            } else {
                // Flow lists don't match or are invalid - synthesize a net flow
                // Build the synthetic flow equation from stock equations
                if let Some(equation) = self.build_synthetic_flow_equation(&stock_equations) {
                    let flow_name = self.generate_net_flow_name(stock_name);
                    flow_info.push(StockFlowInfo {
                        stock_name: stock_name.clone(),
                        inflows: vec![flow_name.clone()],
                        outflows: vec![],
                        synthetic_flow: Some((flow_name, equation)),
                    });
                }
            }
        }

        // Check for unique flow usage across stocks.
        // A flow can only be inflow to one stock and outflow from one stock.
        // If a flow is used by multiple stocks, both stocks get synthetic flows.
        let mut flow_inflow_stock: HashMap<String, String> = HashMap::new(); // flow -> stock
        let mut flow_outflow_stock: HashMap<String, String> = HashMap::new(); // flow -> stock
        let mut stocks_needing_synthetic: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        // First pass: find conflicts
        for info in &flow_info {
            if info.synthetic_flow.is_some() {
                // Already using synthetic flow
                continue;
            }
            for inflow in &info.inflows {
                if let Some(existing_stock) = flow_inflow_stock.get(inflow) {
                    // Conflict: same flow used as inflow by multiple stocks
                    stocks_needing_synthetic.insert(existing_stock.clone());
                    stocks_needing_synthetic.insert(info.stock_name.clone());
                } else {
                    flow_inflow_stock.insert(inflow.clone(), info.stock_name.clone());
                }
            }
            for outflow in &info.outflows {
                if let Some(existing_stock) = flow_outflow_stock.get(outflow) {
                    // Conflict: same flow used as outflow from multiple stocks
                    stocks_needing_synthetic.insert(existing_stock.clone());
                    stocks_needing_synthetic.insert(info.stock_name.clone());
                } else {
                    flow_outflow_stock.insert(outflow.clone(), info.stock_name.clone());
                }
            }
        }

        // Second pass: apply flow information, converting to synthetic where needed
        for mut info in flow_info {
            if stocks_needing_synthetic.contains(&info.stock_name) && info.synthetic_flow.is_none()
            {
                // This stock had a conflict - create synthetic flow
                // Get rate expression from first stock equation
                if let Some(sym) = self.symbols.get(&info.stock_name) {
                    let stock_equations: Vec<&FullEquation<'_>> = sym
                        .equations
                        .iter()
                        .filter(|eq| equation_is_stock(&eq.equation))
                        .collect();
                    if let Some(equation) = self.build_synthetic_flow_equation(&stock_equations) {
                        let flow_name = self.generate_net_flow_name(&info.stock_name);
                        info.synthetic_flow = Some((flow_name.clone(), equation));
                        info.inflows = vec![flow_name];
                        info.outflows = vec![];
                    }
                }
            }

            // Mark existing inflows as flows (skip the synthetic flow itself)
            for inflow in &info.inflows {
                let is_synthetic = info
                    .synthetic_flow
                    .as_ref()
                    .is_some_and(|(n, _)| n == inflow);
                if !is_synthetic && let Some(sym) = self.symbols.get_mut(inflow) {
                    sym.var_type = VariableType::Flow;
                }
            }
            // Mark existing outflows as flows
            for outflow in &info.outflows {
                if let Some(sym) = self.symbols.get_mut(outflow) {
                    sym.var_type = VariableType::Flow;
                }
            }

            // Record synthetic flow if needed
            if let Some((flow_name, equation)) = info.synthetic_flow {
                self.synthetic_flows.push(SyntheticFlow {
                    name: flow_name,
                    equation,
                });
            }

            // Update stock info
            if let Some(sym) = self.symbols.get_mut(&info.stock_name) {
                sym.inflows = info.inflows;
                sym.outflows = info.outflows;
            }
        }
    }

    /// Generate a unique net flow variable name for a stock.
    /// Returns the canonical name (lowercase with spaces) to match symbol table keys.
    fn generate_net_flow_name(&self, stock_name: &str) -> String {
        let base_name = format!("{} net flow", stock_name);
        let canonical_base = canonical_name(&base_name);
        if !self.symbols.contains_key(&canonical_base) {
            return canonical_base;
        }
        // Add suffix for collision avoidance
        let mut suffix = 1;
        loop {
            let name = format!("{} {}", canonical_base, suffix);
            let canonical = canonical_name(&name);
            if !self.symbols.contains_key(&canonical) {
                return canonical;
            }
            suffix += 1;
        }
    }

    /// Build a synthetic flow equation from stock equations.
    /// Returns the appropriate Equation variant (Scalar, ApplyToAll, or Arrayed)
    /// based on whether the stock is arrayed.
    fn build_synthetic_flow_equation(
        &self,
        stock_equations: &[&FullEquation<'_>],
    ) -> Option<Equation> {
        if stock_equations.is_empty() {
            return None;
        }

        // Check if any stock equation has subscripts (arrayed stock)
        let first_eq = stock_equations.first()?;
        let first_lhs = get_lhs(&first_eq.equation)?;

        if first_lhs.subscripts.is_empty() {
            // Scalar stock - extract rate and return Scalar equation
            let rate_str = self
                .extract_integ_rate(&first_eq.equation)
                .map(|e| self.formatter.format_expr(e))?;
            return Some(Equation::Scalar(rate_str, None));
        }

        // Arrayed stock - need to build arrayed equation
        // Collect rate expressions from each equation with their subscripts

        // Expand LHS subscripts to get dimensions and element keys
        let mut all_dims: Option<Vec<String>> = None;
        let mut element_rates: HashMap<String, String> = HashMap::new();

        for eq in stock_equations {
            let lhs = match get_lhs(&eq.equation) {
                Some(l) => l,
                None => continue,
            };

            let rate_str = match self.extract_integ_rate(&eq.equation) {
                Some(e) => self.formatter.format_expr(e),
                None => continue,
            };

            if lhs.subscripts.is_empty() {
                // Mixed scalar/arrayed - shouldn't happen, but fall back to first scalar
                continue;
            }

            // Expand subscripts to get dimension names and element keys
            let mut dims: Vec<String> = Vec::new();
            let mut expanded_elements: Vec<Vec<String>> = Vec::new();

            for s in &lhs.subscripts {
                let sub_name = match s {
                    Subscript::Element(n, _) | Subscript::BangElement(n, _) => n.as_ref(),
                };

                if let Some((dim, elements)) = self.expand_subscript(sub_name) {
                    dims.push(dim);
                    expanded_elements.push(elements);
                } else {
                    // Unknown subscript - skip this equation
                    continue;
                }
            }

            // Check dimensions consistency
            if let Some(ref mut existing_dims) = all_dims {
                // Normalize for comparison
                let normalized_existing: Vec<_> = existing_dims
                    .iter()
                    .map(|d| self.normalize_dimension(d))
                    .collect();
                let normalized_new: Vec<_> =
                    dims.iter().map(|d| self.normalize_dimension(d)).collect();
                if normalized_existing != normalized_new {
                    // Inconsistent dimensions - skip
                    continue;
                }
                // If the raw dimension names differ but normalized names match,
                // the equations span different subranges of the same parent.
                // Promote to the parent dimensions (but not through
                // equivalences -- alias dimensions should keep their own name).
                if *existing_dims != dims {
                    *existing_dims = existing_dims
                        .iter()
                        .map(|d| self.resolve_subrange_to_parent(d))
                        .collect();
                }
            } else {
                all_dims = Some(dims);
            }

            // Compute element keys and store rate
            let element_keys = cartesian_product(&expanded_elements);
            for key in element_keys {
                element_rates.insert(key, rate_str.clone());
            }
        }

        let dims = all_dims?;
        if dims.is_empty() {
            return None;
        }

        // Format dimension names -- dims are already normalized to
        // parent dimensions when equations span different subranges
        let formatted_dims: Vec<String> = dims
            .iter()
            .map(|d| self.get_formatted_dimension_name(d))
            .collect();

        // Convert to elements vector for Arrayed equation
        let mut elements: Vec<(String, String, Option<String>, Option<GraphicalFunction>)> =
            element_rates
                .into_iter()
                .map(|(key, rate)| (key, rate, None, None))
                .collect();
        elements.sort_by(|a, b| a.0.cmp(&b.0));

        if elements.is_empty() {
            return None;
        }

        Some(Equation::Arrayed(formatted_dims, elements))
    }

    /// Extract the rate expression from an INTEG call.
    fn extract_integ_rate<'a>(&self, eq: &'a MdlEquation<'input>) -> Option<&'a Expr<'input>> {
        match eq {
            MdlEquation::Regular(_, expr) => self.extract_integ_rate_expr(expr),
            _ => None,
        }
    }

    fn extract_integ_rate_expr<'a>(&self, expr: &'a Expr<'input>) -> Option<&'a Expr<'input>> {
        match expr {
            Expr::App(name, _, args, CallKind::Builtin, _) if to_lower_space(name) == "integ" => {
                if !args.is_empty() {
                    return Some(&args[0]);
                }
                None
            }
            Expr::Paren(inner, _) => self.extract_integ_rate_expr(inner),
            _ => None,
        }
    }

    /// Extract flow variable names from a stock's equation.
    /// Returns (inflows, outflows) if the rate expression can be decomposed.
    fn extract_flows_from_equation(
        &self,
        eq: &MdlEquation<'_>,
    ) -> Option<(Vec<String>, Vec<String>)> {
        match eq {
            MdlEquation::Regular(_, expr) => self.extract_flows_from_integ(expr),
            _ => None,
        }
    }

    /// Extract flows from an INTEG expression's rate argument.
    fn extract_flows_from_integ(&self, expr: &Expr<'_>) -> Option<(Vec<String>, Vec<String>)> {
        match expr {
            Expr::App(name, _, args, CallKind::Builtin, _) if to_lower_space(name) == "integ" => {
                if !args.is_empty() {
                    return self.analyze_rate_expression(&args[0]);
                }
                None
            }
            Expr::Paren(inner, _) => self.extract_flows_from_integ(inner),
            _ => None,
        }
    }

    /// Analyze a rate expression to identify inflows and outflows.
    /// Uses the is_all_plus_minus algorithm from xmutil.
    fn analyze_rate_expression(&self, rate: &Expr<'_>) -> Option<(Vec<String>, Vec<String>)> {
        let mut inflows = Vec::new();
        let mut outflows = Vec::new();

        if self.collect_flows(rate, true, &mut inflows, &mut outflows) {
            Some((inflows, outflows))
        } else {
            // Complex expression - can't decompose into simple flows
            None
        }
    }

    /// Recursively collect flow variable names from a rate expression.
    /// Returns false if the expression is too complex (contains mul/div/functions),
    /// or if validity checks fail (duplicates, stock-as-flow, unknown symbols).
    ///
    /// Subscripted flows are allowed to match xmutil's behavior (though xmutil notes
    /// this has a bug where subscripts aren't properly compared).
    fn collect_flows(
        &self,
        expr: &Expr<'_>,
        positive: bool,
        inflows: &mut Vec<String>,
        outflows: &mut Vec<String>,
    ) -> bool {
        match expr {
            Expr::Var(name, _subscripts, _) => {
                let canonical = canonical_name(name);

                // Note: xmutil allows subscripted flows and just checks the variable name
                // without comparing subscripts. This has a known bug (per xmutil comments)
                // where STOCK[A]=INTEG(FLOW[B],0) and STOCK[B]=INTEG(FLOW[A],0) would both
                // use FLOW as their flow, even though the subscripts differ.

                // Duplicate check: same variable appearing twice in same direction
                if positive && inflows.contains(&canonical) {
                    return false;
                }
                if !positive && outflows.contains(&canonical) {
                    return false;
                }

                // Cross-direction check: same variable in both inflows and outflows (e.g., a - a)
                // This is invalid and should trigger synthetic net flow
                if positive && outflows.contains(&canonical) {
                    return false;
                }
                if !positive && inflows.contains(&canonical) {
                    return false;
                }

                // Stock-as-flow check: if this is a stock, can't use as flow
                if let Some(info) = self.symbols.get(&canonical)
                    && info.var_type == VariableType::Stock
                {
                    return false;
                }

                // Unknown symbol check: if we don't know about this symbol,
                // treat as invalid for flow decomposition (could be a constant or error)
                if !self.symbols.contains_key(&canonical) {
                    return false;
                }

                if positive {
                    inflows.push(canonical);
                } else {
                    outflows.push(canonical);
                }
                true
            }
            Expr::Op1(crate::mdl::ast::UnaryOp::Negative, inner, _) => {
                self.collect_flows(inner, !positive, inflows, outflows)
            }
            Expr::Op1(crate::mdl::ast::UnaryOp::Positive, inner, _) => {
                self.collect_flows(inner, positive, inflows, outflows)
            }
            Expr::Op2(BinaryOp::Add, left, right, _) => {
                self.collect_flows(left, positive, inflows, outflows)
                    && self.collect_flows(right, positive, inflows, outflows)
            }
            Expr::Op2(BinaryOp::Sub, left, right, _) => {
                self.collect_flows(left, positive, inflows, outflows)
                    && self.collect_flows(right, !positive, inflows, outflows)
            }
            Expr::Paren(inner, _) => self.collect_flows(inner, positive, inflows, outflows),
            // Anything else (mul, div, functions, constants) means we can't decompose
            _ => false,
        }
    }

    /// Check if an equation is an empty RHS.
    pub(super) fn is_empty_rhs(&self, eq: &MdlEquation<'_>) -> bool {
        matches!(eq, MdlEquation::EmptyRhs(_, _))
    }

    /// Check if an equation is an A FUNCTION OF placeholder.
    pub(super) fn is_afo_expr_in_eq(&self, eq: &MdlEquation<'_>) -> bool {
        match eq {
            MdlEquation::Regular(_, expr) => self.is_afo_expr(expr),
            _ => false,
        }
    }

    /// Check if an expression is A FUNCTION OF.
    fn is_afo_expr(&self, expr: &Expr<'_>) -> bool {
        match expr {
            Expr::App(name, _, _, CallKind::Builtin, _) => to_lower_space(name) == "a function of",
            Expr::Paren(inner, _) => self.is_afo_expr(inner),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::convert_mdl;
    use simlin_core::datamodel::{Equation, GraphicalFunctionKind, Variable};

    #[test]
    fn test_top_level_integ_detected_as_stock() {
        // A top-level INTEG should be detected as a stock
        let mdl = "Stock = INTEG(rate, 100)
~ Units
~ A stock |
rate = 10
~ Units/Time
~ A rate |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let stock = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "stock");
        assert!(
            matches!(stock, Some(Variable::Stock(_))),
            "Top-level INTEG should be a Stock"
        );
    }

    #[test]
    fn test_nested_integ_not_detected_as_stock() {
        // A variable with INTEG nested inside another function should NOT be a stock
        let mdl = "aux = MAX(INTEG(a, 0), INTEG(b, 0))
~ Units
~ An auxiliary with nested INTEGs |
a = 1
~ Units
~ |
b = 2
~ Units
~ |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let aux = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "aux");
        assert!(
            matches!(aux, Some(Variable::Aux(_))),
            "Nested INTEG should NOT make this a Stock, got: {:?}",
            aux
        );
    }

    #[test]
    fn test_parenthesized_integ_detected_as_stock() {
        // INTEG wrapped in parens should still be detected as a stock
        let mdl = "Stock = (INTEG(rate, 100))
~ Units
~ A stock |
rate = 10
~ Units/Time
~ |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let stock = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "stock");
        assert!(
            matches!(stock, Some(Variable::Stock(_))),
            "Parenthesized INTEG should be a Stock"
        );
    }

    #[test]
    fn test_integ_in_binary_op_not_detected_as_stock() {
        // A variable with INTEG in a binary operation should NOT be a stock
        // This is because x + INTEG(...) is not just INTEG(...) - it's an expression
        // that includes INTEG as part of a larger computation
        let mdl = "aux = x + INTEG(a, 0)
~ Units
~ An auxiliary |
x = 1
~ Units
~ |
a = 1
~ Units
~ |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let aux = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "aux");
        // IMPORTANT: current code DOES detect INTEG in binary ops as stock.
        // But per the plan, this should NOT be a stock - only top-level INTEG should be.
        // For now we're documenting current behavior. The plan P0-C says to fix this.
        // Let's assert what SHOULD happen according to the plan:
        assert!(
            matches!(aux, Some(Variable::Aux(_))),
            "INTEG in binary op should NOT make this a Stock, got: {:?}",
            aux
        );
    }

    #[test]
    fn test_integ_in_if_then_else_not_detected_as_stock() {
        // IF THEN ELSE(cond, INTEG(a, 0), INTEG(b, 0)) should NOT be a stock
        // Only top-level INTEG creates stocks
        let mdl = "aux = IF THEN ELSE(cond, INTEG(a, 0), INTEG(b, 0))
~ Units
~ An auxiliary with conditional INTEGs |
cond = 1
~ ~|
a = 1
~ Units
~ |
b = 2
~ Units
~ |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let aux = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "aux");
        assert!(
            matches!(aux, Some(Variable::Aux(_))),
            "INTEG in IF THEN ELSE should NOT make this a Stock, got: {:?}",
            aux
        );
    }

    #[test]
    fn test_stock_with_constant_rate_synthesizes_net_flow() {
        // Stock = INTEG(10, 0) -> creates "stock net flow" with equation 10
        let mdl = "stock = INTEG(10, 0)
~ Units
~ A stock with constant rate |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        // The stock should have a synthesized net flow
        let stock = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "stock");
        assert!(
            matches!(stock, Some(Variable::Stock(_))),
            "Should be a Stock"
        );

        if let Some(Variable::Stock(s)) = stock {
            assert!(
                !s.inflows.is_empty() || !s.outflows.is_empty(),
                "Stock should have synthesized flow: inflows={:?}, outflows={:?}",
                s.inflows,
                s.outflows
            );
        }

        // There should be a synthetic flow variable
        let net_flow = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident().contains("net_flow"));
        assert!(
            net_flow.is_some(),
            "Should have synthesized net flow variable, got: {:?}",
            project.models[0]
                .variables
                .iter()
                .map(|v| v.get_ident())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_stock_with_complex_rate_synthesizes_net_flow() {
        // Stock = INTEG(a * b, init) -> creates "stock net flow" with equation a * b
        let mdl = "stock = INTEG(a * b, 0)
~ Units
~ A stock with complex rate |
a = 2
~ Units
~ |
b = 3
~ Units
~ |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let stock = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "stock");
        if let Some(Variable::Stock(s)) = stock {
            assert!(
                !s.inflows.is_empty() || !s.outflows.is_empty(),
                "Stock should have synthesized flow for complex rate"
            );
        } else {
            panic!("Expected Stock variable");
        }
    }

    #[test]
    fn test_stock_with_decomposable_rate_uses_named_flows() {
        // Stock = INTEG(inflow - outflow, 100) -> uses existing named flows
        let mdl = "stock = INTEG(inflow - outflow, 100)
~ Units
~ A stock |
inflow = 10
~ Units/Time
~ |
outflow = 5
~ Units/Time
~ |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let stock = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "stock");
        if let Some(Variable::Stock(s)) = stock {
            assert_eq!(s.inflows, vec!["inflow"]);
            assert_eq!(s.outflows, vec!["outflow"]);
        } else {
            panic!("Expected Stock variable");
        }
    }

    #[test]
    fn test_duplicate_flow_in_rate_fails_decomposition() {
        // INTEG(x + x, 0) has duplicate flow 'x' - should synthesize net flow
        let mdl = "stock = INTEG(x + x, 0)
~ Units
~ Stock with duplicate flow in rate |
x = 1
~ Units/Time
~ |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        // Should have a synthesized net flow because x appears twice
        let net_flow = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident().contains("net_flow"));
        assert!(
            net_flow.is_some(),
            "Should synthesize net flow for duplicate flow in rate"
        );
    }

    #[test]
    fn test_stock_as_flow_fails_decomposition() {
        // INTEG(other_stock, 0) where other_stock is itself a stock - should fail decomposition
        let mdl = "stock1 = INTEG(stock2, 0)
~ Units
~ Stock using another stock as rate |
stock2 = INTEG(rate, 100)
~ Units
~ |
rate = 1
~ Units/Time
~ |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        // stock1 should have a synthesized net flow because stock2 is a stock, not a flow
        let stock1 = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "stock1");
        if let Some(Variable::Stock(s)) = stock1 {
            // The inflows should contain a synthesized flow, not "stock2"
            assert!(
                !s.inflows.contains(&"stock2".to_string()),
                "stock2 should not be used as inflow since it's a stock"
            );
        }
    }

    #[test]
    fn test_lookup_extrapolate_does_not_mark_table() {
        // LOOKUP EXTRAPOLATE performs extrapolation at call time but does NOT mark
        // the table itself as extrapolating (per xmutil behavior).
        // Only TABXL marks the table as extrapolating.
        let mdl = "my_table(
 [(0,0)-(10,10)],(0,0),(10,10))
~ ~|
result = LOOKUP EXTRAPOLATE(my_table, TIME)
~ Units
~ Uses extrapolate at call time |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let my_table = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "my_table");
        assert!(my_table.is_some(), "Should have my_table variable");

        if let Some(Variable::Aux(a)) = my_table {
            assert!(a.gf.is_some(), "my_table should have graphical function");
            let gf = a.gf.as_ref().unwrap();
            // LOOKUP EXTRAPOLATE does NOT mark the table as extrapolating
            assert_eq!(
                gf.kind,
                GraphicalFunctionKind::Continuous,
                "LOOKUP EXTRAPOLATE should not mark table as Extrapolate"
            );
        } else {
            panic!("Expected Aux variable for my_table");
        }
    }

    #[test]
    fn test_tabxl_marks_table_as_extrapolate() {
        // TABXL marks the referenced table as extrapolating (per xmutil behavior).
        let mdl = "my_table(
 [(0,0)-(10,10)],(0,0),(10,10))
~ ~|
result = TABXL(my_table, TIME)
~ Units
~ Uses TABXL which marks table |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let my_table = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "my_table");
        assert!(my_table.is_some(), "Should have my_table variable");

        if let Some(Variable::Aux(a)) = my_table {
            assert!(a.gf.is_some(), "my_table should have graphical function");
            let gf = a.gf.as_ref().unwrap();
            // TABXL DOES mark the table as extrapolating
            assert_eq!(
                gf.kind,
                GraphicalFunctionKind::Extrapolate,
                "TABXL should mark table as Extrapolate"
            );
        } else {
            panic!("Expected Aux variable for my_table");
        }
    }

    #[test]
    fn test_flow_unique_inflow_enforcement() {
        // Same flow used as inflow to two different stocks should trigger synthetic flows
        let mdl = "Stock1 = INTEG(shared_flow, 0)
~ ~|
Stock2 = INTEG(shared_flow, 0)
~ ~|
shared_flow = 5
~ ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        // Since shared_flow is used as inflow to both stocks, xmutil would
        // create synthetic net flows for both stocks instead of linking to shared_flow
        let stock1 = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "stock1")
            .expect("Should have stock1");
        if let Variable::Stock(s) = stock1 {
            // Should have a synthetic flow (contains "net flow"), not shared_flow
            assert!(
                s.inflows.iter().any(|f| f.contains("net flow")),
                "stock1 should have synthetic net flow due to shared inflow: {:?}",
                s.inflows
            );
            // Should NOT have shared_flow directly
            assert!(
                !s.inflows.contains(&"shared flow".to_string()),
                "stock1 should not have shared_flow directly: {:?}",
                s.inflows
            );
        } else {
            panic!("Expected Stock variable");
        }
    }

    #[test]
    fn test_flow_unique_outflow_enforcement() {
        // Same flow used as outflow from two different stocks
        let mdl = "Stock1 = INTEG(-shared_flow, 100)
~ ~|
Stock2 = INTEG(-shared_flow, 100)
~ ~|
shared_flow = 5
~ ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        // Both stocks use shared_flow as outflow - should trigger synthetic flows
        // The synthetic flow becomes an inflow with negative equation, not outflow
        let stock1 = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "stock1")
            .expect("Should have stock1");
        if let Variable::Stock(s) = stock1 {
            // Should have a synthetic flow (in inflows since we use net flow approach)
            assert!(
                s.inflows.iter().any(|f| f.contains("net flow")),
                "stock1 should have synthetic net flow due to shared outflow: inflows={:?}, outflows={:?}",
                s.inflows,
                s.outflows
            );
        } else {
            panic!("Expected Stock variable");
        }
    }

    #[test]
    fn test_arrayed_stock_apply_to_all_synthetic_flow() {
        // Stock[Dim] = INTEG(rate * 2, 0) - synthetic flow should be ApplyToAll
        let mdl = "DimA: a1, a2
~ ~|
Stock[DimA] = INTEG(10, 0)
~ Units ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        let net_flow = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident().contains("net_flow") || v.get_ident().contains("net flow"))
            .expect("Should have synthetic net flow");

        if let Variable::Flow(f) = net_flow {
            match &f.equation {
                Equation::ApplyToAll(dims, _eq_str, _) => {
                    assert_eq!(dims, &["DimA"], "Should have stock's dimensions");
                }
                Equation::Arrayed(dims, elements) => {
                    assert_eq!(dims, &["DimA"], "Should have stock's dimensions");
                    assert_eq!(elements.len(), 2, "Should have 2 elements");
                }
                Equation::Scalar(_, _) => {
                    panic!("Should NOT be scalar for arrayed stock")
                }
            }
        } else {
            panic!("Expected Flow variable, got {:?}", net_flow);
        }
    }

    #[test]
    fn test_arrayed_stock_per_element_synthetic_flow() {
        // Per-element stock equations need per-element synthetic flow
        let mdl = "DimA: a1, a2
~ ~|
Stock[a1] = INTEG(10, 0)
~ Units ~|
Stock[a2] = INTEG(20, 0)
~ Units ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        let net_flow = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident().contains("net_flow") || v.get_ident().contains("net flow"))
            .expect("Should have synthetic net flow");

        if let Variable::Flow(f) = net_flow {
            match &f.equation {
                Equation::Arrayed(dims, elements) => {
                    assert_eq!(dims, &["DimA"]);
                    assert_eq!(elements.len(), 2);

                    let a1_eq = elements.iter().find(|(k, _, _, _)| k == "a1");
                    let a2_eq = elements.iter().find(|(k, _, _, _)| k == "a2");

                    assert_eq!(a1_eq.unwrap().1, "10", "a1 rate should be 10");
                    assert_eq!(a2_eq.unwrap().1, "20", "a2 rate should be 20");
                }
                other => panic!("Expected Arrayed equation, got {:?}", other),
            }
        } else {
            panic!("Expected Flow variable");
        }
    }

    #[test]
    fn test_scalar_stock_scalar_synthetic_flow() {
        // Non-arrayed stock should still get scalar synthetic flow
        let mdl = "stock = INTEG(rate * 2, 0)
~ Units ~|
rate = 5
~ Units/Time ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        let net_flow = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident().contains("net_flow") || v.get_ident().contains("net flow"))
            .expect("Should have synthetic net flow");

        if let Variable::Flow(f) = net_flow {
            assert!(
                matches!(f.equation, Equation::Scalar(_, _)),
                "Scalar stock should have scalar synthetic flow, got {:?}",
                f.equation
            );
        } else {
            panic!("Expected Flow variable");
        }
    }

    #[test]
    fn test_time_units_from_range_only() {
        let mdl = "INITIAL TIME = 0
~ [0, 100]
~ |
FINAL TIME = 100
~ [0, 100]
~ |
TIME STEP = 1
~ [0, 10]
~ |
SAVEPER = 1
~ [0, 10]
~ |
x = 5
~ widgets
~ |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        let project = result.unwrap();
        // Range-only units on control vars should produce "1" for time_units
        assert_eq!(project.sim_specs.time_units.as_deref(), Some("1"));
    }

    #[test]
    fn test_time_units_prefers_explicit_over_range_only() {
        // FINAL TIME has range-only, but TIME STEP has explicit units
        // Should prefer the explicit "Months" over dimensionless "1"
        let mdl = "INITIAL TIME = 0
~ [0, 100]
~ |
FINAL TIME = 100
~ [0, 100]
~ |
TIME STEP = 1
~ Months
~ |
SAVEPER = 1
~ [0, 10]
~ |
x = 5
~ widgets
~ |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        let project = result.unwrap();
        // Explicit units from TIME STEP should be preferred over range-only from FINAL TIME
        assert_eq!(project.sim_specs.time_units.as_deref(), Some("Months"));
    }

    #[test]
    fn test_time_units_time_step_wins_over_final_time() {
        // xmutil priority: TIME STEP > FINAL TIME > INITIAL TIME > "Months"
        // When both TIME STEP and FINAL TIME have units, TIME STEP wins.
        let mdl = "INITIAL TIME = 0
~ [0, 100]
~ |
FINAL TIME = 100
~ Years
~ |
TIME STEP = 1
~ Months
~ |
SAVEPER = 1
~ [0, 10]
~ |
x = 5
~ widgets
~ |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        let project = result.unwrap();
        // TIME STEP has highest priority, so "Months" wins over "Years"
        assert_eq!(project.sim_specs.time_units.as_deref(), Some("Months"));
    }

    #[test]
    fn test_time_units_time_step_only_has_units() {
        // When only TIME STEP has explicit units
        let mdl = "INITIAL TIME = 0
~ [0, 100]
~ |
FINAL TIME = 100
~ ~|
TIME STEP = 1
~ Months
~ |
SAVEPER = 1
~ [0, 10]
~ |
x = 5
~ widgets
~ |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        let project = result.unwrap();
        // TIME STEP's explicit units are used (FINAL TIME has no units at all)
        assert_eq!(project.sim_specs.time_units.as_deref(), Some("Months"));
    }

    #[test]
    fn test_time_units_final_time_only_has_units() {
        // When only FINAL TIME has explicit units
        let mdl = "INITIAL TIME = 0
~ [0, 100]
~ |
FINAL TIME = 100
~ Years
~ |
TIME STEP = 1
~ ~|
SAVEPER = 1
~ [0, 10]
~ |
x = 5
~ widgets
~ |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        let project = result.unwrap();
        // FINAL TIME's explicit units are used
        assert_eq!(project.sim_specs.time_units.as_deref(), Some("Years"));
    }

    #[test]
    fn test_time_units_range_only_time_step_wins() {
        // TIME STEP has range-only "1", FINAL TIME has explicit "Years"
        // TIME STEP still wins because it has higher priority (even with range-only)
        let mdl = "INITIAL TIME = 0
~ ~|
FINAL TIME = 100
~ Years
~ |
TIME STEP = 1
~ [0, 10]
~ |
SAVEPER = 1
~ ~|
x = 5
~ widgets
~ |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        let project = result.unwrap();
        assert_eq!(project.sim_specs.time_units.as_deref(), Some("1"));
    }

    #[test]
    fn test_time_units_no_units_anywhere_defaults_to_months() {
        // No control variable has units at all -> falls back to "Months"
        let mdl = "INITIAL TIME = 0
~ ~|
FINAL TIME = 100
~ ~|
TIME STEP = 1
~ ~|
SAVEPER = 1
~ ~|
x = 5
~ widgets
~ |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        let project = result.unwrap();
        assert_eq!(project.sim_specs.time_units.as_deref(), Some("Months"));
    }

    #[test]
    fn test_time_units_initial_time_fallback() {
        // Only INITIAL TIME has units -> used as last resort before "Months"
        let mdl = "INITIAL TIME = 0
~ Days
~ |
FINAL TIME = 100
~ ~|
TIME STEP = 1
~ ~|
SAVEPER = 1
~ ~|
x = 5
~ widgets
~ |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        let project = result.unwrap();
        assert_eq!(project.sim_specs.time_units.as_deref(), Some("Days"));
    }
}
