// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! MDL AST to datamodel conversion.
//!
//! This module converts parsed MDL AST items directly to `datamodel::Project`,
//! bypassing the XMILE intermediate format.

use std::collections::{HashMap, HashSet};

use simlin_core::datamodel::{
    self, Dimension, DimensionElements, Dt, Equation, GraphicalFunction, GraphicalFunctionKind,
    GraphicalFunctionScale, Model, Project, SimMethod, SimSpecs, Unit, Variable, Visibility,
};

use crate::mdl::ast::{
    BinaryOp, CallKind, Equation as MdlEquation, Expr, FullEquation, Lhs, LookupTable, MdlItem,
    Subscript, SubscriptElement,
};
use crate::mdl::builtins::to_lower_space;
use crate::mdl::reader::EquationReader;
use crate::mdl::xmile_compat::{XmileFormatter, format_unit_expr, space_to_underbar};

/// Errors that can occur during MDL to datamodel conversion.
#[derive(Debug)]
#[allow(dead_code)]
pub enum ConvertError {
    /// Reader error during parsing
    Reader(crate::mdl::reader::ReaderError),
    /// Invalid subscript range specification
    InvalidRange(String),
    /// Cyclic dimension definition detected (e.g., DimA: DimB, DimB: DimA)
    CyclicDimensionDefinition(String),
    /// Other conversion error
    Other(String),
}

impl From<crate::mdl::reader::ReaderError> for ConvertError {
    fn from(e: crate::mdl::reader::ReaderError) -> Self {
        ConvertError::Reader(e)
    }
}

/// Type of variable determined during conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VariableType {
    Stock,
    Flow,
    Aux,
}

/// Information about a symbol collected during the first pass.
#[derive(Debug)]
struct SymbolInfo<'input> {
    /// The parsed equation(s) for this symbol
    equations: Vec<FullEquation<'input>>,
    /// Detected variable type
    var_type: VariableType,
    /// For stocks: list of inflow variable names
    inflows: Vec<String>,
    /// For stocks: list of outflow variable names
    outflows: Vec<String>,
    /// Whether this is a "unwanted" variable (control var)
    unwanted: bool,
    /// Alternate name for XMILE output (e.g., "DT" for "TIME STEP")
    alternate_name: Option<String>,
}

impl<'input> SymbolInfo<'input> {
    fn new() -> Self {
        SymbolInfo {
            equations: Vec::new(),
            var_type: VariableType::Aux,
            inflows: Vec::new(),
            outflows: Vec::new(),
            unwanted: false,
            alternate_name: None,
        }
    }
}

/// A synthetic flow variable generated for stocks with non-decomposable rates.
struct SyntheticFlow {
    /// Canonical name of the flow
    name: String,
    /// The full equation (Scalar, ApplyToAll, or Arrayed)
    equation: Equation,
}

/// Context for MDL to datamodel conversion.
#[allow(dead_code)]
pub struct ConversionContext<'input> {
    /// All parsed items from the MDL file
    items: Vec<MdlItem<'input>>,
    /// Symbol table mapping canonical names to symbol info
    symbols: HashMap<String, SymbolInfo<'input>>,
    /// Collected dimensions
    dimensions: Vec<Dimension>,
    /// Dimension equivalences: source -> target
    equivalences: HashMap<String, String>,
    /// SimSpecs builder
    sim_specs: SimSpecsBuilder,
    /// Integration method (to be used in Phase 10 for settings parsing)
    integration_method: SimMethod,
    /// Unit equivalences
    unit_equivs: Vec<Unit>,
    /// Expression formatter
    formatter: XmileFormatter,
    /// Synthetic flows generated for stocks with non-decomposable rates
    synthetic_flows: Vec<SyntheticFlow>,
    /// Maps element names to their owning dimension name (canonical).
    /// Used for ambiguous element resolution and BangElement formatting.
    /// First/largest match wins like xmutil's owner assignment.
    element_owners: HashMap<String, String>,
    /// Maps dimension names (canonical) to their element lists (expanded).
    dimension_elements: HashMap<String, Vec<String>>,
    /// Lookup variables that should use extrapolation (from LOOKUP EXTRAPOLATE calls)
    extrapolate_lookups: HashSet<String>,
    /// Raw subscript definitions for recursive expansion during dimension building.
    /// Maps canonical dimension name to the raw SubscriptElement list.
    raw_subscript_defs: HashMap<String, Vec<SubscriptElement<'input>>>,
}

/// Builder for SimSpecs extracted from control variables.
#[derive(Default)]
struct SimSpecsBuilder {
    start: Option<f64>,
    stop: Option<f64>,
    dt: Option<f64>,
    save_step: Option<f64>,
    time_units: Option<String>,
}

impl SimSpecsBuilder {
    fn build(self) -> SimSpecs {
        SimSpecs {
            start: self.start.unwrap_or(0.0),
            stop: self.stop.unwrap_or(200.0),
            dt: self.dt.map(Dt::Dt).unwrap_or_default(),
            // Saveper defaults to dt if not specified (per xmutil behavior)
            save_step: self.save_step.or(self.dt).map(Dt::Dt),
            sim_method: SimMethod::Euler,
            // Default to "Months" to match xmutil
            time_units: self.time_units.or_else(|| Some("Months".to_string())),
        }
    }
}

impl<'input> ConversionContext<'input> {
    /// Create a new conversion context from MDL source.
    pub fn new(source: &'input str) -> Result<Self, ConvertError> {
        let reader = EquationReader::new(source);
        let items: Result<Vec<MdlItem<'input>>, _> = reader.collect();
        let items = items?;

        Ok(ConversionContext {
            items,
            symbols: HashMap::new(),
            dimensions: Vec::new(),
            equivalences: HashMap::new(),
            sim_specs: SimSpecsBuilder::default(),
            integration_method: SimMethod::Euler,
            unit_equivs: Vec::new(),
            formatter: XmileFormatter::new(),
            synthetic_flows: Vec::new(),
            element_owners: HashMap::new(),
            dimension_elements: HashMap::new(),
            extrapolate_lookups: HashSet::new(),
            raw_subscript_defs: HashMap::new(),
        })
    }

    /// Convert the MDL to a Project.
    pub fn convert(mut self) -> Result<Project, ConvertError> {
        // Pass 1: Collect symbols and build initial symbol table
        self.collect_symbols();

        // Pass 2: Build dimensions from subscript definitions
        self.build_dimensions()?;

        // Pass 2.5: Set subrange dimensions on formatter for bang-subscript formatting
        // Subranges are dimensions with maps_to set (they map to a parent dimension)
        let subrange_dims: HashSet<String> = self
            .dimensions
            .iter()
            .filter(|d| d.maps_to.is_some())
            .map(|d| d.name.clone())
            .collect();
        self.formatter.set_subranges(subrange_dims);

        // Pass 3: Mark variable types (stock/flow/aux) and extract control vars
        self.mark_variable_types();

        // Pass 4: Scan for LOOKUP EXTRAPOLATE usage
        self.scan_for_extrapolate_lookups();

        // Pass 5: Link stocks and flows
        self.link_stocks_and_flows();

        // Pass 6: Build the final project
        self.build_project()
    }

    /// Pass 1: Collect all symbols from the parsed items.
    fn collect_symbols(&mut self) {
        for item in &self.items {
            match item {
                MdlItem::Equation(eq) => {
                    if let Some(name) = get_equation_name(&eq.equation) {
                        let canonical = canonical_name(&name);
                        let info = self
                            .symbols
                            .entry(canonical)
                            .or_insert_with(SymbolInfo::new);
                        info.equations.push((**eq).clone());
                    }
                }
                MdlItem::Group(_) | MdlItem::Macro(_) | MdlItem::EqEnd(_) => {}
            }
        }
    }

    /// Pass 2: Build dimensions from subscript definitions.
    fn build_dimensions(&mut self) -> Result<(), ConvertError> {
        // Phase 1a: Collect ALL raw subscript definitions (before expansion)
        for item in &self.items {
            if let MdlItem::Equation(eq) = item
                && let MdlEquation::SubscriptDef(name, def) = &eq.equation
            {
                let canonical = canonical_name(name);
                self.raw_subscript_defs
                    .insert(canonical, def.elements.clone());
            }
        }

        // Phase 1b: Collect equivalences
        for item in &self.items {
            if let MdlItem::Equation(eq) = item
                && let MdlEquation::Equivalence(src, dst, _) = &eq.equation
            {
                let src_canonical = canonical_name(src);
                let dst_canonical = canonical_name(dst);
                self.equivalences.insert(src_canonical, dst_canonical);
            }
        }

        // Phase 2: Build dimensions using recursive expansion
        // We need to collect the subscript def names first to avoid borrowing issues
        let subscript_names: Vec<(String, String)> = self
            .items
            .iter()
            .filter_map(|item| {
                if let MdlItem::Equation(eq) = item
                    && let MdlEquation::SubscriptDef(name, _) = &eq.equation
                {
                    Some((name.to_string(), canonical_name(name)))
                } else {
                    None
                }
            })
            .collect();

        for (original_name, canonical) in subscript_names {
            let dim = self.build_dimension_recursive(&original_name, &canonical)?;
            self.dimensions.push(dim);
        }

        // Phase 3: build dimension_elements map
        for dim in &self.dimensions {
            if let DimensionElements::Named(elements) = &dim.elements {
                let dim_canonical = canonical_name(&dim.name);
                self.dimension_elements
                    .insert(dim_canonical, elements.clone());
            }
        }

        // Phase 4: establish element ownership (larger dimension owns elements)
        let mut dims_by_size: Vec<_> = self
            .dimension_elements
            .iter()
            .map(|(name, elems)| (name.clone(), elems.clone()))
            .collect();
        dims_by_size.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

        for (dim_name, elements) in dims_by_size {
            for elem in elements {
                let elem_canonical = canonical_name(&elem);
                self.element_owners
                    .entry(elem_canonical)
                    .or_insert_with(|| dim_name.clone());
            }
        }

        // Phase 5: materialize equivalence dimensions as actual Dimension entries
        // and add them to dimension_elements for expand_subscript to find them
        for (src, dst) in &self.equivalences {
            if let Some(target_dim) = self
                .dimensions
                .iter()
                .find(|d| canonical_name(&d.name) == *dst)
            {
                let alias = Dimension {
                    name: space_to_underbar(src),
                    elements: target_dim.elements.clone(),
                    maps_to: Some(dst.clone()),
                };
                // Add alias to dimension_elements so expand_subscript can find it
                if let DimensionElements::Named(elements) = &alias.elements {
                    self.dimension_elements
                        .insert(src.clone(), elements.clone());
                }
                self.dimensions.push(alias);
            }
        }

        Ok(())
    }

    /// Build a single dimension with recursive element expansion.
    /// If an element name matches a dimension name, expand to that dimension's elements.
    fn build_dimension_recursive(
        &self,
        original_name: &str,
        canonical: &str,
    ) -> Result<Dimension, ConvertError> {
        let mut visited = std::collections::HashSet::new();
        let elements = self.expand_subscript_elements(canonical, &mut visited)?;

        // Get mapping info from raw subscript def
        let maps_to = self.get_dimension_mapping(original_name);

        Ok(Dimension {
            name: space_to_underbar(original_name),
            elements: DimensionElements::Named(elements),
            maps_to,
        })
    }

    /// Get the maps_to target for a dimension from its SubscriptDef.
    fn get_dimension_mapping(&self, name: &str) -> Option<String> {
        for item in &self.items {
            if let MdlItem::Equation(eq) = item
                && let MdlEquation::SubscriptDef(def_name, def) = &eq.equation
                && canonical_name(def_name) == canonical_name(name)
            {
                return def.mapping.as_ref().and_then(|m| {
                    if m.entries.len() != 1 {
                        return None;
                    }
                    match &m.entries[0] {
                        crate::mdl::ast::MappingEntry::Name(n, _) => Some(canonical_name(n)),
                        crate::mdl::ast::MappingEntry::DimensionMapping { dimension, .. } => {
                            Some(canonical_name(dimension))
                        }
                        _ => None,
                    }
                });
            }
        }
        None
    }

    /// Recursively expand subscript elements.
    /// If an element name matches a dimension, expand to that dimension's elements.
    fn expand_subscript_elements(
        &self,
        dim_canonical: &str,
        visited: &mut std::collections::HashSet<String>,
    ) -> Result<Vec<String>, ConvertError> {
        // Cycle detection
        if visited.contains(dim_canonical) {
            return Err(ConvertError::CyclicDimensionDefinition(
                dim_canonical.to_string(),
            ));
        }

        let elements = match self.raw_subscript_defs.get(dim_canonical) {
            Some(elems) => elems.clone(),
            None => return Ok(vec![]), // Unknown dimension
        };

        visited.insert(dim_canonical.to_string());

        let mut result = Vec::new();
        for elem in &elements {
            match elem {
                SubscriptElement::Element(e, _) => {
                    let elem_canonical = canonical_name(e);
                    // Check if this element is actually a dimension reference
                    if self.raw_subscript_defs.contains_key(&elem_canonical) {
                        // Recursively expand
                        let expanded = self.expand_subscript_elements(&elem_canonical, visited)?;
                        result.extend(expanded);
                    } else {
                        // Plain element
                        result.push(space_to_underbar(e));
                    }
                }
                SubscriptElement::Range(start, end, _) => {
                    let expanded = expand_range(start, end)?;
                    result.extend(expanded);
                }
            }
        }

        visited.remove(dim_canonical);
        Ok(result)
    }

    /// Resolve an element subscript to its owning dimension.
    /// Returns the canonical dimension name if found.
    #[allow(dead_code)]
    fn resolve_element_to_dimension(&self, element: &str) -> Option<&str> {
        self.element_owners
            .get(&canonical_name(element))
            .map(|s| s.as_str())
    }

    /// Pass 3: Mark variable types based on equation content.
    fn mark_variable_types(&mut self) {
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
                && let Some(eq) = info.equations.first()
            {
                if let Some(value) = extract_constant_value(&eq.equation) {
                    match *name {
                        "initial time" => self.sim_specs.start = Some(value),
                        "final time" => self.sim_specs.stop = Some(value),
                        "time step" => self.sim_specs.dt = Some(value),
                        "saveper" => self.sim_specs.save_step = Some(value),
                        _ => {}
                    }
                }

                // Extract time units from TIME STEP or FINAL TIME
                if (*name == "time step" || *name == "final time")
                    && self.sim_specs.time_units.is_none()
                    && let Some(units) = &eq.units
                    && let Some(expr) = &units.expr
                {
                    self.sim_specs.time_units = Some(format_unit_expr(expr));
                }
            }
        }

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
    fn scan_for_extrapolate_lookups(&mut self) {
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
    fn link_stocks_and_flows(&mut self) {
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
            let info = self.symbols.get(stock_name).unwrap();

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
            if let Some(ref existing_dims) = all_dims {
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

        // Format dimensions
        let formatted_dims: Vec<String> = dims.iter().map(|d| space_to_underbar(d)).collect();

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

    /// Pass 5: Build the final Project.
    fn build_project(self) -> Result<Project, ConvertError> {
        let mut variables: Vec<Variable> = Vec::new();

        for (name, info) in &self.symbols {
            // Skip unwanted variables (control vars)
            if info.unwanted {
                continue;
            }

            // Skip subscript definitions (they're in dimensions)
            let is_subscript_def = info.equations.iter().any(|eq| {
                matches!(
                    eq.equation,
                    MdlEquation::SubscriptDef(_, _) | MdlEquation::Equivalence(_, _, _)
                )
            });
            if is_subscript_def {
                continue;
            }

            // Check if this variable has element-specific equations (x[a]=1; x[b]=2)
            // If so, build an Arrayed equation from all element-specific equations
            if let Some(var) = self.build_variable_with_elements(name, info) {
                variables.push(var);
            } else {
                // Fall back to single-equation handling
                // PurgeAFOEq: If multiple equations and first is A FUNCTION OF or EmptyRhs, skip it
                let eq = self.select_equation(&info.equations);
                if let Some(eq) = eq
                    && let Some(var) = self.build_variable(name, info, eq)
                {
                    variables.push(var);
                }
            }
        }

        // Add synthetic flow variables
        for synthetic in &self.synthetic_flows {
            let flow = Variable::Flow(datamodel::Flow {
                ident: space_to_underbar(&synthetic.name),
                equation: synthetic.equation.clone(),
                documentation: String::new(),
                units: None,
                gf: None,
                non_negative: false,
                can_be_module_input: false,
                visibility: Visibility::Private,
                ai_state: None,
                uid: None,
            });
            variables.push(flow);
        }

        // Sort variables by canonical name for deterministic output
        variables.sort_by_key(|a| canonical_name(a.get_ident()));

        let model = Model {
            name: "main".to_string(),
            sim_specs: None,
            variables,
            views: vec![],
            loop_metadata: vec![],
        };

        Ok(Project {
            name: String::new(),
            sim_specs: self.sim_specs.build(),
            dimensions: self.dimensions,
            units: self.unit_equivs,
            models: vec![model],
            source: None,
            ai_information: None,
        })
    }

    /// Select the appropriate equation from a list, implementing PurgeAFOEq logic.
    ///
    /// This matches xmutil's `PurgeAFOEq` algorithm:
    /// 1. First pass: drop all equations with no expression (EmptyRhs)
    /// 2. Second pass: if multiple equations remain and first is AFO, drop it
    /// 3. Return first remaining equation
    fn select_equation<'a>(
        &self,
        equations: &'a [FullEquation<'input>],
    ) -> Option<&'a FullEquation<'input>> {
        if equations.is_empty() {
            return None;
        }

        // First pass: collect non-empty equations
        let non_empty: Vec<&FullEquation<'input>> = equations
            .iter()
            .filter(|eq| !self.is_empty_rhs(&eq.equation))
            .collect();

        if non_empty.is_empty() {
            // All equations are empty, return the first one anyway
            return Some(&equations[0]);
        }

        if non_empty.len() == 1 {
            return Some(non_empty[0]);
        }

        // Multiple non-empty equations: check if first is A FUNCTION OF
        if self.is_afo_expr_in_eq(&non_empty[0].equation) {
            // Skip the AFO placeholder, use the next equation
            return Some(non_empty[1]);
        }

        Some(non_empty[0])
    }

    /// Check if an equation has an empty RHS.
    fn is_empty_rhs(&self, eq: &MdlEquation<'_>) -> bool {
        matches!(eq, MdlEquation::EmptyRhs(_, _))
    }

    /// Check if an equation is an A FUNCTION OF placeholder.
    fn is_afo_expr_in_eq(&self, eq: &MdlEquation<'_>) -> bool {
        match eq {
            MdlEquation::Regular(_, expr) => self.is_afo_expr(expr),
            _ => false,
        }
    }

    /// Get the elements of a dimension, or just the element itself if it's already an element.
    /// Returns (formatted_dimension_name, list_of_elements).
    fn expand_subscript(&self, sub_name: &str) -> Option<(String, Vec<String>)> {
        let canonical = canonical_name(sub_name);

        // If it's an element, return just that element and its owning dimension (formatted)
        if let Some(dim_canonical) = self.element_owners.get(&canonical) {
            // Look up the original dimension name for proper casing
            let formatted_dim = self.get_formatted_dimension_name(dim_canonical);
            return Some((formatted_dim, vec![space_to_underbar(sub_name)]));
        }

        // If it's a dimension, return all its elements
        if self.dimension_elements.contains_key(&canonical) {
            // Look up the original dimension name for proper casing
            let formatted_dim = self.get_formatted_dimension_name(&canonical);
            let elements = self.dimension_elements.get(&canonical)?;
            return Some((formatted_dim, elements.clone()));
        }

        None
    }

    /// Get the formatted dimension name (space_to_underbar) from a canonical name.
    fn get_formatted_dimension_name(&self, canonical: &str) -> String {
        // Find the original dimension name and format it
        for dim in &self.dimensions {
            if canonical_name(&dim.name) == canonical {
                return space_to_underbar(&dim.name);
            }
        }
        // Fallback to canonical if not found
        space_to_underbar(canonical)
    }

    /// Normalize a dimension name through equivalences to its canonical target.
    /// For example, if DimA <-> DimB, normalize_dimension("dima") returns "dimb".
    fn normalize_dimension(&self, dim: &str) -> String {
        let canonical = canonical_name(dim);
        // Follow equivalence chain to target
        self.equivalences
            .get(&canonical)
            .cloned()
            .unwrap_or(canonical)
    }

    /// Check if an equation is a NumberList or TabbedArray (which have special handling).
    fn is_number_list_or_tabbed(&self, eq: &MdlEquation<'_>) -> bool {
        matches!(
            eq,
            MdlEquation::NumberList(_, _) | MdlEquation::TabbedArray(_, _)
        )
    }

    /// Build a variable with element-specific equations if applicable.
    /// Returns None if not element-specific (should use normal handling).
    ///
    /// This function handles:
    /// - Single element-specific equations (P1): x[a1] = 5
    /// - Apply-to-all with element overrides (P2): x[DimA] = 1, x[a1] = 2
    /// - Mixed element/dimension subscripts (High): x[a1, DimB] = expr
    ///
    /// NumberList and TabbedArray equations are excluded - they have special handling
    /// in build_equation that handles their multi-value RHS correctly.
    fn build_variable_with_elements(&self, name: &str, info: &SymbolInfo<'_>) -> Option<Variable> {
        // Filter to get valid equations (not empty, not AFO, not number list/tabbed)
        let valid_eqs: Vec<&FullEquation<'_>> = info
            .equations
            .iter()
            .filter(|eq| !self.is_empty_rhs(&eq.equation))
            .filter(|eq| !self.is_afo_expr_in_eq(&eq.equation))
            .filter(|eq| !self.is_number_list_or_tabbed(&eq.equation))
            .collect();

        if valid_eqs.is_empty() {
            return None;
        }

        // Classify equations and determine parent dimensions
        struct ExpandedEquation<'a> {
            eq: &'a FullEquation<'a>,
            element_keys: Vec<String>, // Cartesian product of expanded subscripts
        }

        let mut expanded_eqs: Vec<ExpandedEquation<'_>> = Vec::new();
        let mut parent_dims: Option<Vec<String>> = None;
        let mut has_subscripted_eq = false;

        for eq in &valid_eqs {
            if let Some(lhs) = get_lhs(&eq.equation) {
                if lhs.subscripts.is_empty() {
                    // Scalar equation - can't mix with subscripted
                    if has_subscripted_eq {
                        return None;
                    }
                    continue;
                }

                has_subscripted_eq = true;

                // Expand each subscript (element → single element, dimension → all elements)
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
                        // Unknown subscript - fall back to normal handling
                        return None;
                    }
                }

                // Verify all equations have the same parent dimensions (normalizing via equivalences)
                if let Some(ref existing_dims) = parent_dims {
                    // Normalize both sets of dimensions through equivalences for comparison
                    let normalized_existing: Vec<_> = existing_dims
                        .iter()
                        .map(|d| self.normalize_dimension(d))
                        .collect();
                    let normalized_new: Vec<_> =
                        dims.iter().map(|d| self.normalize_dimension(d)).collect();
                    if normalized_existing != normalized_new {
                        // Inconsistent dimensions - can't form a proper array
                        return None;
                    }
                } else {
                    parent_dims = Some(dims);
                }

                // Compute Cartesian product of expanded elements
                let element_keys = cartesian_product(&expanded_elements);
                expanded_eqs.push(ExpandedEquation { eq, element_keys });
            } else {
                return None;
            }
        }

        // If no subscripted equations found, use normal handling
        if !has_subscripted_eq {
            return None;
        }

        let parent_dims = parent_dims?;
        if parent_dims.is_empty() {
            return None;
        }

        // Build element map: key -> (equation_string, comment, gf)
        // Later equations override earlier ones (element-specific overrides apply-to-all)
        let mut element_map: HashMap<String, (String, Option<String>, Option<GraphicalFunction>)> =
            HashMap::new();

        for exp_eq in expanded_eqs {
            let (eq_str, gf) = self.build_equation_rhs(
                name,
                &exp_eq.eq.equation,
                info.var_type == VariableType::Stock,
            );
            let comment = exp_eq.eq.comment.as_ref().map(|c| c.to_string());

            for key in exp_eq.element_keys {
                element_map.insert(key, (eq_str.clone(), comment.clone(), gf.clone()));
            }
        }

        // Convert map to sorted vector
        let mut elements: Vec<(String, String, Option<String>, Option<GraphicalFunction>)> =
            element_map
                .into_iter()
                .map(|(key, (eq_str, comment, gf))| (key, eq_str, comment, gf))
                .collect();
        elements.sort_by(|a, b| a.0.cmp(&b.0));

        if elements.is_empty() {
            return None;
        }

        // Use formatted dimension names (spaces to underscores)
        let formatted_dims: Vec<String> =
            parent_dims.iter().map(|d| space_to_underbar(d)).collect();

        let equation = Equation::Arrayed(formatted_dims.clone(), elements);

        // Build the variable
        let ident = space_to_underbar(name);
        let first_eq = valid_eqs.first()?;
        let documentation = first_eq
            .comment
            .as_ref()
            .map(|c| c.to_string())
            .unwrap_or_default();
        let units = first_eq
            .units
            .as_ref()
            .and_then(|u| u.expr.as_ref())
            .map(format_unit_expr);

        match info.var_type {
            VariableType::Stock => Some(Variable::Stock(datamodel::Stock {
                ident,
                equation,
                documentation,
                units,
                inflows: info.inflows.clone(),
                outflows: info.outflows.clone(),
                non_negative: false,
                can_be_module_input: false,
                visibility: Visibility::Private,
                ai_state: None,
                uid: None,
            })),
            VariableType::Flow => Some(Variable::Flow(datamodel::Flow {
                ident,
                equation,
                documentation,
                units,
                gf: None,
                non_negative: false,
                can_be_module_input: false,
                visibility: Visibility::Private,
                ai_state: None,
                uid: None,
            })),
            VariableType::Aux => Some(Variable::Aux(datamodel::Aux {
                ident,
                equation,
                documentation,
                units,
                gf: None,
                can_be_module_input: false,
                visibility: Visibility::Private,
                ai_state: None,
                uid: None,
            })),
        }
    }

    /// Build the equation RHS string, handling stock initial values.
    /// The var_name is used for extrapolation detection on lookups.
    fn build_equation_rhs(
        &self,
        var_name: &str,
        eq: &MdlEquation<'_>,
        is_stock: bool,
    ) -> (String, Option<GraphicalFunction>) {
        match eq {
            MdlEquation::Regular(_, expr) => {
                if is_stock && let Some(initial) = self.extract_integ_initial(expr) {
                    return (self.formatter.format_expr(initial), None);
                }
                (self.formatter.format_expr(expr), None)
            }
            MdlEquation::Lookup(_, table) => {
                // For lookups, return empty string - the GF will be attached
                (
                    String::new(),
                    Some(self.build_graphical_function(var_name, table)),
                )
            }
            MdlEquation::WithLookup(_, input, table) => (
                self.formatter.format_expr(input),
                Some(self.build_graphical_function(var_name, table)),
            ),
            _ => (String::new(), None),
        }
    }

    /// Check if an expression is an A FUNCTION OF call.
    fn is_afo_expr(&self, expr: &Expr<'_>) -> bool {
        match expr {
            Expr::App(name, _, _, CallKind::Builtin, _) => to_lower_space(name) == "a function of",
            Expr::Paren(inner, _) => self.is_afo_expr(inner),
            _ => false,
        }
    }

    /// Build a Variable from symbol info.
    fn build_variable(
        &self,
        name: &str,
        info: &SymbolInfo<'_>,
        eq: &FullEquation<'_>,
    ) -> Option<Variable> {
        let ident = space_to_underbar(name);
        let documentation = eq
            .comment
            .as_ref()
            .map(|c| c.to_string())
            .unwrap_or_default();
        let units = eq
            .units
            .as_ref()
            .and_then(|u| u.expr.as_ref())
            .map(format_unit_expr);

        match info.var_type {
            VariableType::Stock => {
                let (equation, _gf) = self.build_equation(&eq.equation, true);
                Some(Variable::Stock(datamodel::Stock {
                    ident,
                    equation,
                    documentation,
                    units,
                    inflows: info.inflows.clone(),
                    outflows: info.outflows.clone(),
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: Visibility::Private,
                    ai_state: None,
                    uid: None,
                }))
            }
            VariableType::Flow => {
                let (equation, gf) = self.build_equation(&eq.equation, false);
                Some(Variable::Flow(datamodel::Flow {
                    ident,
                    equation,
                    documentation,
                    units,
                    gf,
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: Visibility::Private,
                    ai_state: None,
                    uid: None,
                }))
            }
            VariableType::Aux => {
                let (equation, gf) = self.build_equation(&eq.equation, false);
                Some(Variable::Aux(datamodel::Aux {
                    ident,
                    equation,
                    documentation,
                    units,
                    gf,
                    can_be_module_input: false,
                    visibility: Visibility::Private,
                    ai_state: None,
                    uid: None,
                }))
            }
        }
    }

    /// Build an Equation from an MDL equation.
    /// For stocks, extract the initial value from INTEG.
    fn build_equation(
        &self,
        eq: &MdlEquation<'_>,
        is_stock: bool,
    ) -> (Equation, Option<GraphicalFunction>) {
        match eq {
            MdlEquation::Regular(lhs, expr) => {
                if is_stock {
                    // For stocks, extract initial value from INTEG
                    if let Some(initial) = self.extract_integ_initial(expr) {
                        let initial_str = self.formatter.format_expr(initial);
                        return (self.make_equation(lhs, &initial_str), None);
                    }
                }
                let eq_str = self.formatter.format_expr(expr);
                (self.make_equation(lhs, &eq_str), None)
            }
            MdlEquation::Lookup(lhs, table) => {
                let gf = self.build_graphical_function(&lhs.name, table);
                (self.make_equation(lhs, ""), Some(gf))
            }
            MdlEquation::WithLookup(lhs, input, table) => {
                let gf = self.build_graphical_function(&lhs.name, table);
                let input_str = self.formatter.format_expr(input);
                (self.make_equation(lhs, &input_str), Some(gf))
            }
            MdlEquation::EmptyRhs(lhs, _) => (self.make_equation(lhs, ""), None),
            MdlEquation::Implicit(lhs) => {
                // Implicit equations become lookups on TIME with a default table
                let gf = self.make_default_lookup();
                (self.make_equation(lhs, "TIME"), Some(gf))
            }
            MdlEquation::Data(lhs, expr) => {
                let eq_str = expr
                    .as_ref()
                    .map(|e| self.formatter.format_expr(e))
                    .unwrap_or_default();
                (self.make_equation(lhs, &eq_str), None)
            }
            MdlEquation::TabbedArray(lhs, values) | MdlEquation::NumberList(lhs, values) => {
                // Create an arrayed equation from the number list
                self.make_array_equation(lhs, values)
            }
            MdlEquation::SubscriptDef(_, _) | MdlEquation::Equivalence(_, _, _) => {
                (Equation::Scalar(String::new(), None), None)
            }
        }
    }

    /// Create a default lookup table for implicit equations.
    /// Per xmutil: `(0,1),(1,1)` is the default table when tbl is NULL.
    fn make_default_lookup(&self) -> GraphicalFunction {
        GraphicalFunction {
            kind: GraphicalFunctionKind::Continuous,
            x_points: Some(vec![0.0, 1.0]),
            y_points: vec![1.0, 1.0],
            x_scale: GraphicalFunctionScale { min: 0.0, max: 1.0 },
            y_scale: GraphicalFunctionScale { min: 0.0, max: 2.0 },
        }
    }

    /// Create an Equation from LHS and equation string, handling subscripts.
    fn make_equation(&self, lhs: &Lhs<'_>, eq_str: &str) -> Equation {
        if lhs.subscripts.is_empty() {
            Equation::Scalar(eq_str.to_string(), None)
        } else {
            // Subscripted equation becomes ApplyToAll
            let dims: Vec<String> = lhs
                .subscripts
                .iter()
                .map(|s| match s {
                    Subscript::Element(name, _) | Subscript::BangElement(name, _) => {
                        space_to_underbar(name)
                    }
                })
                .collect();
            Equation::ApplyToAll(dims, eq_str.to_string(), None)
        }
    }

    /// Create an arrayed equation from a list of values.
    ///
    /// For TabbedArray and NumberList, we need to create element-specific equations.
    /// This requires knowing the dimension elements to map values to subscripts.
    fn make_array_equation(
        &self,
        lhs: &Lhs<'_>,
        values: &[f64],
    ) -> (Equation, Option<GraphicalFunction>) {
        if lhs.subscripts.is_empty() {
            // Scalar case: just use the first value (shouldn't normally happen)
            let eq_str = if !values.is_empty() {
                format_number(values[0])
            } else {
                String::new()
            };
            return (Equation::Scalar(eq_str, None), None);
        }

        // Get dimension names from subscripts
        let dims: Vec<String> = lhs
            .subscripts
            .iter()
            .map(|s| match s {
                Subscript::Element(name, _) | Subscript::BangElement(name, _) => {
                    space_to_underbar(name)
                }
            })
            .collect();

        // Look up dimension elements to create Arrayed equation
        let elements = match self.get_dimension_elements(&dims) {
            Some(e) => e,
            None => {
                // Dimension not found - fall back to ApplyToAll with first value
                let eq_str = if !values.is_empty() {
                    format_number(values[0])
                } else {
                    String::new()
                };
                return (Equation::ApplyToAll(dims, eq_str, None), None);
            }
        };

        if elements.len() != values.len() {
            // Length mismatch - this is an error, but for now fall back to ApplyToAll
            // TODO: Consider returning an error instead
            let eq_str = if !values.is_empty() {
                format_number(values[0])
            } else {
                String::new()
            };
            return (Equation::ApplyToAll(dims, eq_str, None), None);
        }

        // Create element-specific equations
        let element_eqs: Vec<(String, String, Option<String>, Option<GraphicalFunction>)> =
            elements
                .into_iter()
                .zip(values.iter())
                .map(|(elem, &val)| (elem, format_number(val), None, None))
                .collect();

        (Equation::Arrayed(dims, element_eqs), None)
    }

    /// Get dimension elements for the given dimension names.
    ///
    /// For single-dimensional arrays, returns the elements of that dimension.
    /// For multi-dimensional arrays, returns the Cartesian product of elements
    /// in row-major order (first dimension varies slowest).
    ///
    /// Returns `None` if any dimension is not found.
    fn get_dimension_elements(&self, dim_names: &[String]) -> Option<Vec<String>> {
        if dim_names.is_empty() {
            return None;
        }

        // Look up elements for each dimension (case-insensitive)
        let mut dim_elements: Vec<Vec<String>> = Vec::with_capacity(dim_names.len());
        for dim_name in dim_names {
            let canonical_dim = canonical_name(dim_name);
            let mut found = false;
            for dim in &self.dimensions {
                // Compare canonicalized names for case-insensitive matching
                if canonical_name(&dim.name) == canonical_dim
                    && let DimensionElements::Named(elements) = &dim.elements
                {
                    dim_elements.push(elements.clone());
                    found = true;
                    break;
                }
            }
            if !found {
                return None;
            }
        }

        // Compute Cartesian product in row-major order
        if dim_elements.len() == 1 {
            // Single dimension: just return the elements
            return Some(dim_elements.into_iter().next().unwrap());
        }

        // Multi-dimensional: compute Cartesian product
        Some(cartesian_product(&dim_elements))
    }

    /// Extract the initial value expression from an INTEG call.
    fn extract_integ_initial<'a>(&self, expr: &'a Expr<'input>) -> Option<&'a Expr<'input>> {
        match expr {
            Expr::App(name, _, args, CallKind::Builtin, _) if to_lower_space(name) == "integ" => {
                // INTEG(rate, initial) - return the initial value
                if args.len() >= 2 {
                    return Some(&args[1]);
                }
                None
            }
            Expr::Paren(inner, _) => self.extract_integ_initial(inner),
            _ => None,
        }
    }

    /// Build a GraphicalFunction from a LookupTable.
    /// The var_name is used to check if LOOKUP EXTRAPOLATE was used with this lookup.
    fn build_graphical_function(&self, var_name: &str, table: &LookupTable) -> GraphicalFunction {
        // Handle legacy XY format by transforming if needed
        let (x_vals, y_vals) = if table.format == crate::mdl::ast::TableFormat::LegacyXY {
            // Legacy format: values are stored flat in x_vals, need to split
            let n = table.x_vals.len() / 2;
            if n > 0 && table.x_vals.len().is_multiple_of(2) {
                (table.x_vals[..n].to_vec(), table.x_vals[n..].to_vec())
            } else {
                (table.x_vals.clone(), table.y_vals.clone())
            }
        } else {
            (table.x_vals.clone(), table.y_vals.clone())
        };
        let (x_scale, y_scale) =
            if let (Some(x_range), Some(y_range)) = (table.x_range, table.y_range) {
                (
                    GraphicalFunctionScale {
                        min: x_range.0,
                        max: x_range.1,
                    },
                    GraphicalFunctionScale {
                        min: y_range.0,
                        max: y_range.1,
                    },
                )
            } else {
                // Derive from data
                let x_min = x_vals.iter().cloned().fold(f64::INFINITY, f64::min);
                let x_max = x_vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                let y_min = y_vals.iter().cloned().fold(f64::INFINITY, f64::min);
                let y_max = y_vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                (
                    GraphicalFunctionScale {
                        min: x_min,
                        max: x_max,
                    },
                    GraphicalFunctionScale {
                        min: y_min,
                        max: y_max,
                    },
                )
            };

        // Check if extrapolation should be enabled:
        // 1. table.extrapolate is set in the lookup definition itself
        // 2. OR the lookup is used with LOOKUP EXTRAPOLATE / TABXL somewhere
        let should_extrapolate =
            table.extrapolate || self.extrapolate_lookups.contains(&canonical_name(var_name));

        GraphicalFunction {
            kind: if should_extrapolate {
                GraphicalFunctionKind::Extrapolate
            } else {
                GraphicalFunctionKind::Continuous
            },
            x_points: if x_vals.is_empty() {
                None
            } else {
                Some(x_vals)
            },
            y_points: y_vals,
            x_scale,
            y_scale,
        }
    }
}

/// Format a number for equation output.
fn format_number(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }

    let abs = value.abs();
    if (1e-4..1e6).contains(&abs) {
        let s = format!("{}", value);
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        }
    } else {
        format!("{:e}", value)
    }
}

/// Convert a name to canonical form (lowercase with spaces).
fn canonical_name(name: &str) -> String {
    to_lower_space(name)
}

/// Get the name from an equation's LHS.
fn get_equation_name(eq: &MdlEquation<'_>) -> Option<String> {
    match eq {
        MdlEquation::Regular(lhs, _)
        | MdlEquation::EmptyRhs(lhs, _)
        | MdlEquation::Implicit(lhs)
        | MdlEquation::Lookup(lhs, _)
        | MdlEquation::WithLookup(lhs, _, _)
        | MdlEquation::Data(lhs, _)
        | MdlEquation::TabbedArray(lhs, _)
        | MdlEquation::NumberList(lhs, _) => Some(lhs.name.to_string()),
        MdlEquation::SubscriptDef(name, _) => Some(name.to_string()),
        MdlEquation::Equivalence(name, _, _) => Some(name.to_string()),
    }
}

/// Get the LHS from an equation if it has one.
fn get_lhs<'a, 'input>(eq: &'a MdlEquation<'input>) -> Option<&'a Lhs<'input>> {
    match eq {
        MdlEquation::Regular(lhs, _)
        | MdlEquation::EmptyRhs(lhs, _)
        | MdlEquation::Implicit(lhs)
        | MdlEquation::Lookup(lhs, _)
        | MdlEquation::WithLookup(lhs, _, _)
        | MdlEquation::Data(lhs, _)
        | MdlEquation::TabbedArray(lhs, _)
        | MdlEquation::NumberList(lhs, _) => Some(lhs),
        MdlEquation::SubscriptDef(_, _) | MdlEquation::Equivalence(_, _, _) => None,
    }
}

/// Check if an equation has a top-level INTEG call (making it a stock).
///
/// Only the root expression determines stock type. An auxiliary like
/// `x = MAX(INTEG(a, 0), INTEG(b, 0))` or `x = a + INTEG(b, 0)` should NOT
/// be marked as a stock - only `x = INTEG(rate, init)` should.
/// Parens are allowed: `x = (INTEG(rate, init))` is also a stock.
fn equation_is_stock(eq: &MdlEquation<'_>) -> bool {
    match eq {
        MdlEquation::Regular(_, expr) => is_top_level_integ(expr),
        _ => false,
    }
}

/// Check if an expression is a top-level INTEG call.
/// Only checks the root expression, allowing parens but not nested in other constructs.
fn is_top_level_integ(expr: &Expr<'_>) -> bool {
    match expr {
        Expr::App(name, _, _, CallKind::Builtin, _) => to_lower_space(name) == "integ",
        Expr::Paren(inner, _) => is_top_level_integ(inner),
        _ => false,
    }
}

/// Extract a constant value from an equation if it's a simple constant.
fn extract_constant_value(eq: &MdlEquation<'_>) -> Option<f64> {
    match eq {
        MdlEquation::Regular(_, expr) => extract_expr_constant(expr),
        _ => None,
    }
}

/// Compute the Cartesian product of multiple vectors in row-major order.
///
/// For example: `[[a, b], [1, 2]]` produces `["a, 1", "a, 2", "b, 1", "b, 2"]`
/// (first dimension varies slowest).
fn cartesian_product(dim_elements: &[Vec<String>]) -> Vec<String> {
    if dim_elements.is_empty() {
        return vec![];
    }
    if dim_elements.len() == 1 {
        return dim_elements[0].clone();
    }

    // Start with the first dimension
    let mut result: Vec<Vec<&str>> = dim_elements[0].iter().map(|e| vec![e.as_str()]).collect();

    // Multiply in each subsequent dimension
    for dim in &dim_elements[1..] {
        let mut new_result = Vec::with_capacity(result.len() * dim.len());
        for prefix in &result {
            for elem in dim {
                let mut new_combo = prefix.clone();
                new_combo.push(elem.as_str());
                new_result.push(new_combo);
            }
        }
        result = new_result;
    }

    // Join each combination into a comma-separated string (no spaces - compiler expects "a,b" not "a, b")
    result.into_iter().map(|combo| combo.join(",")).collect()
}

fn extract_expr_constant(expr: &Expr<'_>) -> Option<f64> {
    match expr {
        Expr::Const(v, _) => Some(*v),
        Expr::Op1(crate::mdl::ast::UnaryOp::Negative, inner, _) => {
            extract_expr_constant(inner).map(|v| -v)
        }
        Expr::Paren(inner, _) => extract_expr_constant(inner),
        _ => None,
    }
}

/// Expand a numeric range like (A1-A10) to individual elements.
fn expand_range(start: &str, end: &str) -> Result<Vec<String>, ConvertError> {
    // Find where numeric suffix starts
    let start_num_pos = start
        .rfind(|c: char| !c.is_ascii_digit())
        .map(|i| i + 1)
        .unwrap_or(0);
    let end_num_pos = end
        .rfind(|c: char| !c.is_ascii_digit())
        .map(|i| i + 1)
        .unwrap_or(0);

    let start_prefix = &start[..start_num_pos];
    let end_prefix = &end[..end_num_pos];

    if start_prefix != end_prefix || start_num_pos != end_num_pos {
        return Err(ConvertError::InvalidRange(format!(
            "Bad subscript range specification: {} - {}",
            start, end
        )));
    }

    let low: u32 = start[start_num_pos..]
        .parse()
        .map_err(|_| ConvertError::InvalidRange(format!("Invalid range start: {}", start)))?;
    let high: u32 = end[end_num_pos..]
        .parse()
        .map_err(|_| ConvertError::InvalidRange(format!("Invalid range end: {}", end)))?;

    if low >= high {
        return Err(ConvertError::InvalidRange(format!(
            "Bad subscript range specification: {} >= {}",
            low, high
        )));
    }

    Ok((low..=high)
        .map(|n| format!("{}{}", start_prefix, n))
        .collect())
}

/// Convert MDL source to a Project.
pub fn convert_mdl(source: &str) -> Result<Project, ConvertError> {
    let ctx = ConversionContext::new(source)?;
    ctx.convert()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_range() {
        let result = expand_range("A1", "A5").unwrap();
        assert_eq!(result, vec!["A1", "A2", "A3", "A4", "A5"]);
    }

    #[test]
    fn test_expand_range_two_digit() {
        let result = expand_range("Item10", "Item15").unwrap();
        assert_eq!(
            result,
            vec!["Item10", "Item11", "Item12", "Item13", "Item14", "Item15"]
        );
    }

    #[test]
    fn test_expand_range_mismatch() {
        let result = expand_range("A1", "B5");
        assert!(result.is_err());
    }

    #[test]
    fn test_expand_range_invalid_order() {
        let result = expand_range("A5", "A1");
        assert!(result.is_err());
    }

    #[test]
    fn test_canonical_name() {
        assert_eq!(canonical_name("My Variable"), "my variable");
        assert_eq!(canonical_name("INITIAL TIME"), "initial time");
    }

    #[test]
    fn test_simple_conversion() {
        let mdl = "x = 5
~ Units
~ A constant |
y = x * 2
~ Units
~ Derived value |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();
        assert_eq!(project.models.len(), 1);
        assert!(!project.models[0].variables.is_empty());
    }

    #[test]
    fn test_stock_conversion() {
        let mdl = "Stock = INTEG(inflow - outflow, 100)
~ Units
~ A stock |
inflow = 10
~ Units/Time
~ Inflow rate |
outflow = 5
~ Units/Time
~ Outflow rate |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        // Find the stock
        let stock = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "stock");
        assert!(stock.is_some(), "Should have a stock variable");

        if let Some(Variable::Stock(s)) = stock {
            assert_eq!(s.inflows, vec!["inflow"]);
            assert_eq!(s.outflows, vec!["outflow"]);
        } else {
            panic!("Expected Stock variable");
        }
    }

    #[test]
    fn test_subscripted_equation_expands_to_arrayed() {
        // Subscripted equations with dimension subscripts are expanded to Arrayed
        // so that element-specific overrides can be properly merged.
        let mdl = "DimA: a1, a2, a3
~ ~|
x[DimA] = 5
~ Units
~ An arrayed constant |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        // Find x
        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x");
        assert!(x.is_some(), "Should have x variable");

        if let Some(Variable::Aux(a)) = x {
            match &a.equation {
                Equation::Arrayed(dims, elements) => {
                    assert_eq!(dims, &["DimA"]);
                    assert_eq!(elements.len(), 3);
                    // All elements have the same equation "5"
                    for (key, eq, _, _) in elements {
                        assert!(["a1", "a2", "a3"].contains(&key.as_str()));
                        assert_eq!(eq, "5");
                    }
                }
                other => panic!("Expected Arrayed equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_implicit_equation_creates_lookup() {
        let mdl = "data
~ Units
~ Exogenous data |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        // Find data
        let data = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "data");
        assert!(data.is_some(), "Should have data variable");

        if let Some(Variable::Aux(a)) = data {
            assert!(a.gf.is_some(), "Should have graphical function");
            let gf = a.gf.as_ref().unwrap();
            assert_eq!(gf.x_points, Some(vec![0.0, 1.0]));
            assert_eq!(gf.y_points, vec![1.0, 1.0]);
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_number_list_conversion() {
        let mdl = "DimA: a1, a2, a3
~ ~|
x[DimA] = 1, 2, 3
~ Units
~ Array values |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        // Find x
        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x");
        assert!(x.is_some(), "Should have x variable");

        if let Some(Variable::Aux(a)) = x {
            match &a.equation {
                Equation::Arrayed(dims, elements) => {
                    assert_eq!(dims, &["DimA"]);
                    assert_eq!(elements.len(), 3);
                    assert_eq!(elements[0].0, "a1");
                    assert_eq!(elements[0].1, "1");
                    assert_eq!(elements[1].0, "a2");
                    assert_eq!(elements[1].1, "2");
                    assert_eq!(elements[2].0, "a3");
                    assert_eq!(elements[2].1, "3");
                }
                other => panic!("Expected Arrayed equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_a_function_of_purging() {
        // When multiple equations and first is A FUNCTION OF, use the next one
        let mdl = "x = A FUNCTION OF(y)
~ ~|
x = y * 2
~ Units
~ Real equation |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        // Find x
        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x");
        assert!(x.is_some(), "Should have x variable");

        if let Some(Variable::Aux(a)) = x {
            match &a.equation {
                Equation::Scalar(eq, _) => {
                    assert_eq!(eq, "y * 2");
                }
                other => panic!("Expected Scalar equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_multiple_empty_equations_purged() {
        // Multiple empty equations followed by a real one: use the real one
        // [EmptyRhs, EmptyRhs, Regular] -> uses Regular
        let mdl = "x =
~ ~|
x =
~ ~|
x = 42
~ Units
~ Real equation |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x");
        assert!(x.is_some(), "Should have x variable");

        if let Some(Variable::Aux(a)) = x {
            match &a.equation {
                Equation::Scalar(eq, _) => {
                    assert_eq!(eq, "42", "Should use the real equation, not empty");
                }
                other => panic!("Expected Scalar equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_empty_then_afo_then_regular() {
        // [EmptyRhs, AFO, Regular] -> uses Regular
        let mdl = "x =
~ ~|
x = A FUNCTION OF(y)
~ ~|
x = 42
~ Units
~ Real equation |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x");
        assert!(x.is_some(), "Should have x variable");

        if let Some(Variable::Aux(a)) = x {
            match &a.equation {
                Equation::Scalar(eq, _) => {
                    assert_eq!(eq, "42", "Should use the real equation");
                }
                other => panic!("Expected Scalar equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_regular_equations_first_wins() {
        // [Regular1, Regular2] -> uses Regular1 (no purge needed)
        let mdl = "x = 1
~ Units
~ First |
x = 2
~ Units
~ Second |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x");
        assert!(x.is_some(), "Should have x variable");

        if let Some(Variable::Aux(a)) = x {
            match &a.equation {
                Equation::Scalar(eq, _) => {
                    assert_eq!(eq, "1", "First regular equation should win");
                }
                other => panic!("Expected Scalar equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_format_number() {
        assert_eq!(format_number(0.0), "0");
        assert_eq!(format_number(42.0), "42");
        assert_eq!(format_number(3.125), "3.125");
        assert_eq!(format_number(1.5), "1.5");
        // Scientific notation for very large/small
        assert!(format_number(1e10).contains('e'));
        assert!(format_number(1e-10).contains('e'));
    }

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
    fn test_2d_number_list_conversion() {
        // 2D array: x[DimA, DimB] = 1, 2, 3, 4
        // with DimA: a1, a2 and DimB: b1, b2
        // -> [(a1,b1)=1, (a1,b2)=2, (a2,b1)=3, (a2,b2)=4] (row-major order)
        let mdl = "DimA: a1, a2
~ ~|
DimB: b1, b2
~ ~|
x[DimA, DimB] = 1, 2, 3, 4
~ Units
~ 2D array values |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x");
        assert!(x.is_some(), "Should have x variable");

        if let Some(Variable::Aux(a)) = x {
            match &a.equation {
                Equation::Arrayed(dims, elements) => {
                    assert_eq!(dims, &["DimA", "DimB"]);
                    assert_eq!(elements.len(), 4, "Should have 4 elements");
                    // Check row-major order: a1,b1=1, a1,b2=2, a2,b1=3, a2,b2=4
                    // Keys use comma without space to match compiler expectations
                    assert_eq!(elements[0].0, "a1,b1");
                    assert_eq!(elements[0].1, "1");
                    assert_eq!(elements[1].0, "a1,b2");
                    assert_eq!(elements[1].1, "2");
                    assert_eq!(elements[2].0, "a2,b1");
                    assert_eq!(elements[2].1, "3");
                    assert_eq!(elements[3].0, "a2,b2");
                    assert_eq!(elements[3].1, "4");
                }
                other => panic!("Expected Arrayed equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_1d_number_list_still_works() {
        // 1D: x[DimA] = 1, 2, 3 with DimA: a1, a2, a3 -> [(a1, 1), (a2, 2), (a3, 3)]
        let mdl = "DimA: a1, a2, a3
~ ~|
x[DimA] = 1, 2, 3
~ Units
~ Array values |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x");
        if let Some(Variable::Aux(a)) = x {
            match &a.equation {
                Equation::Arrayed(dims, elements) => {
                    assert_eq!(dims, &["DimA"]);
                    assert_eq!(elements.len(), 3);
                    assert_eq!(elements[0].0, "a1");
                    assert_eq!(elements[0].1, "1");
                    assert_eq!(elements[1].0, "a2");
                    assert_eq!(elements[1].1, "2");
                    assert_eq!(elements[2].0, "a3");
                    assert_eq!(elements[2].1, "3");
                }
                other => panic!("Expected Arrayed equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_element_ownership_larger_dimension_wins() {
        // When an element appears in multiple dimensions, the larger dimension
        // should own it (matching xmutil's ownership assignment)
        let mdl = "DimLarge: a, b, c, d
~ ~|
DimSmall: a, b
~ ~|
x = 1
~ ~|
\\\\\\---///
";
        let ctx = ConversionContext::new(mdl).unwrap();
        let mut ctx = ctx;
        ctx.collect_symbols();
        ctx.build_dimensions().unwrap();

        // Element 'a' should be owned by DimLarge (larger dimension)
        assert_eq!(
            ctx.resolve_element_to_dimension("a"),
            Some("dimlarge"),
            "Element 'a' should be owned by the larger dimension"
        );
        assert_eq!(
            ctx.resolve_element_to_dimension("b"),
            Some("dimlarge"),
            "Element 'b' should be owned by the larger dimension"
        );
        // Elements only in DimLarge should still be owned by DimLarge
        assert_eq!(
            ctx.resolve_element_to_dimension("c"),
            Some("dimlarge"),
            "Element 'c' should be owned by DimLarge"
        );
    }

    #[test]
    fn test_equivalence_creates_dimension_alias() {
        // Equivalence `DimA <-> DimB` should create DimA as an alias for DimB
        let mdl = "DimB: x1, x2, x3
~ ~|
DimA <-> DimB
~ ~|
y = 1
~ ~|
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        // Should have both DimA and DimB dimensions
        assert!(
            project.dimensions.iter().any(|d| d.name == "DimB"),
            "Should have DimB dimension"
        );
        assert!(
            project.dimensions.iter().any(|d| d.name == "dima"),
            "Should have DimA dimension (alias)"
        );

        // DimA should map to DimB
        let dim_a = project.dimensions.iter().find(|d| d.name == "dima");
        if let Some(dim) = dim_a {
            assert_eq!(
                dim.maps_to,
                Some("dimb".to_string()),
                "DimA should map to dimb"
            );
        }
    }

    #[test]
    fn test_explicit_dimension_mapping() {
        // Explicit mapping `DimA: a1, a2, a3 -> (DimB: b1, b2, b3)` should extract DimB as maps_to
        let mdl = "DimB: b1, b2, b3
~ ~|
DimA: a1, a2, a3 -> (DimB: b1, b2, b3)
~ ~|
x = 1
~ ~|
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        // DimA should exist with maps_to = dimb
        let dim_a = project.dimensions.iter().find(|d| d.name == "DimA");
        assert!(dim_a.is_some(), "Should have DimA dimension");
        if let Some(dim) = dim_a {
            assert_eq!(
                dim.maps_to,
                Some("dimb".to_string()),
                "DimA should map to dimb via explicit mapping"
            );
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
    fn test_single_element_specific_equation() {
        // P1: A single element-specific equation like x[a1] = 5 should produce
        // an Arrayed equation, not ApplyToAll with "a1" as a dimension.
        let mdl = "DimA: a1, a2, a3
~ ~|
x[a1] = 5
~ Units
~ Single element definition |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x");
        assert!(x.is_some(), "Should have x variable");

        if let Some(Variable::Aux(a)) = x {
            match &a.equation {
                Equation::Arrayed(dims, elements) => {
                    assert_eq!(dims, &["DimA"]);
                    assert_eq!(elements.len(), 1);
                    assert_eq!(elements[0].0, "a1");
                    assert_eq!(elements[0].1, "5");
                }
                other => panic!("Expected Arrayed equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_element_override_with_apply_to_all() {
        // P2: Apply-to-all with element-specific overrides should merge correctly.
        // x[DimA] = 1 (default), x[a1] = 2 (override) -> a1=2, a2=1, a3=1
        let mdl = "DimA: a1, a2, a3
~ ~|
x[DimA] = 1
~ Units
~ Default |
x[a1] = 2
~ Units
~ Override for a1 |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x");
        assert!(x.is_some(), "Should have x variable");

        if let Some(Variable::Aux(a)) = x {
            match &a.equation {
                Equation::Arrayed(dims, elements) => {
                    assert_eq!(dims, &["DimA"]);
                    assert_eq!(elements.len(), 3);

                    // Find each element
                    let a1_eq = elements.iter().find(|(k, _, _, _)| k == "a1");
                    let a2_eq = elements.iter().find(|(k, _, _, _)| k == "a2");
                    let a3_eq = elements.iter().find(|(k, _, _, _)| k == "a3");

                    assert!(a1_eq.is_some(), "Should have a1");
                    assert!(a2_eq.is_some(), "Should have a2");
                    assert!(a3_eq.is_some(), "Should have a3");

                    // a1 should be overridden to "2"
                    assert_eq!(a1_eq.unwrap().1, "2", "a1 should be overridden to 2");
                    // a2 and a3 should have default "1"
                    assert_eq!(a2_eq.unwrap().1, "1", "a2 should have default 1");
                    assert_eq!(a3_eq.unwrap().1, "1", "a3 should have default 1");
                }
                other => panic!("Expected Arrayed equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_mixed_element_dimension_subscripts() {
        // High: Mixed element/dimension subscripts like x[a1, DimB] should expand.
        let mdl = "DimA: a1, a2
~ ~|
DimB: b1, b2
~ ~|
x[a1, DimB] = 5
~ Units
~ Element in first position, dimension in second |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x");
        assert!(x.is_some(), "Should have x variable");

        if let Some(Variable::Aux(a)) = x {
            match &a.equation {
                Equation::Arrayed(dims, elements) => {
                    assert_eq!(dims, &["DimA", "DimB"]);
                    // a1 * (b1, b2) = 2 elements
                    assert_eq!(elements.len(), 2);

                    // Elements should be "a1,b1" and "a1,b2"
                    let keys: Vec<&str> = elements.iter().map(|(k, _, _, _)| k.as_str()).collect();
                    assert!(keys.contains(&"a1,b1"), "Should have a1,b1");
                    assert!(keys.contains(&"a1,b2"), "Should have a1,b2");
                }
                other => panic!("Expected Arrayed equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    // H2: Nested Dimension Expansion tests

    #[test]
    fn test_nested_dimension_definition_expands() {
        // DimA: DimB should expand to DimB's elements
        let mdl = "DimB: x1, x2, x3
~ ~|
DimA: DimB
~ ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        let dim_a = project
            .dimensions
            .iter()
            .find(|d| d.name == "DimA")
            .unwrap();
        if let DimensionElements::Named(elements) = &dim_a.elements {
            assert_eq!(
                elements,
                &["x1", "x2", "x3"],
                "DimA should have DimB's elements"
            );
        } else {
            panic!("Expected Named elements");
        }
    }

    #[test]
    fn test_chained_dimension_expansion() {
        // DimA: DimB, DimB: DimC - should recursively expand
        let mdl = "DimC: c1, c2
~ ~|
DimB: DimC
~ ~|
DimA: DimB
~ ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        let dim_a = project
            .dimensions
            .iter()
            .find(|d| d.name == "DimA")
            .unwrap();
        if let DimensionElements::Named(elements) = &dim_a.elements {
            assert_eq!(
                elements,
                &["c1", "c2"],
                "DimA should recursively expand through DimB to DimC"
            );
        } else {
            panic!("Expected Named elements");
        }
    }

    #[test]
    fn test_forward_reference_dimension_expansion() {
        // DimA: DimB defined BEFORE DimB: x1, x2 - forward reference
        let mdl = "DimA: DimB
~ ~|
DimB: x1, x2
~ ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        let dim_a = project
            .dimensions
            .iter()
            .find(|d| d.name == "DimA")
            .unwrap();
        if let DimensionElements::Named(elements) = &dim_a.elements {
            assert_eq!(elements, &["x1", "x2"], "Forward reference should resolve");
        } else {
            panic!("Expected Named elements");
        }
    }

    #[test]
    fn test_cyclic_dimension_definition_errors() {
        // DimA: DimB, DimB: DimA - cycle should error, not infinite loop
        let mdl = "DimA: DimB
~ ~|
DimB: DimA
~ ~|
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_err(), "Cyclic dimension definition should error");
    }

    #[test]
    fn test_mixed_elements_and_dimension_reference() {
        // DimA: x1, DimB, x4 where DimB: x2, x3 - should be [x1, x2, x3, x4]
        let mdl = "DimB: x2, x3
~ ~|
DimA: x1, DimB, x4
~ ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        let dim_a = project
            .dimensions
            .iter()
            .find(|d| d.name == "DimA")
            .unwrap();
        if let DimensionElements::Named(elements) = &dim_a.elements {
            assert_eq!(elements, &["x1", "x2", "x3", "x4"], "Should expand inline");
        } else {
            panic!("Expected Named elements");
        }
    }

    // P2: Alias Dimension tests

    #[test]
    fn test_alias_dimension_recognized_in_expand_subscript() {
        // DimA <-> DimB, then x[DimA] should work
        let mdl = "DimB: b1, b2
~ ~|
DimA <-> DimB
~ ~|
x[DimA] = 1
~ Units ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x")
            .expect("Should have x variable");

        // Should be recognized and processed correctly
        if let Variable::Aux(a) = x {
            match &a.equation {
                Equation::Arrayed(dims, elements) => {
                    assert_eq!(dims, &["dima"]);
                    assert_eq!(elements.len(), 2);
                }
                other => panic!("Expected Arrayed (dimension expansion), got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_alias_dimension_with_element_override() {
        // DimA <-> DimB, x[DimA]=1, x[b1]=2 should work
        // Note: b1 is owned by DimB, but DimA is an alias, so should be compatible
        let mdl = "DimB: b1, b2
~ ~|
DimA <-> DimB
~ ~|
x[DimA] = 1
~ Units ~|
x[b1] = 2
~ Units ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x")
            .expect("Should have x variable");

        if let Variable::Aux(a) = x {
            match &a.equation {
                Equation::Arrayed(dims, elements) => {
                    // Dimension should be dima (from the first equation)
                    assert_eq!(dims, &["dima"]);
                    assert_eq!(elements.len(), 2);

                    let b1_eq = elements.iter().find(|(k, _, _, _)| k == "b1");
                    let b2_eq = elements.iter().find(|(k, _, _, _)| k == "b2");

                    assert_eq!(b1_eq.unwrap().1, "2", "b1 should be overridden to 2");
                    assert_eq!(b2_eq.unwrap().1, "1", "b2 should have default 1");
                }
                other => panic!("Expected Arrayed, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    // M1: Flow linking improvements tests

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

    // H1: Arrayed Synthetic Flow tests

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

    // L1: SimSpecs saveper fallback

    #[test]
    fn test_simspecs_saveper_defaults_to_dt() {
        // No SAVEPER defined, should default to DT
        let mdl = "INITIAL TIME = 0
~ ~|
FINAL TIME = 100
~ ~|
TIME STEP = 0.5
~ ~|
x = 1
~ ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        assert_eq!(project.sim_specs.dt, Dt::Dt(0.5));
        assert_eq!(
            project.sim_specs.save_step,
            Some(Dt::Dt(0.5)),
            "save_step should default to dt when SAVEPER not defined"
        );
    }

    #[test]
    fn test_simspecs_saveper_explicit() {
        // SAVEPER explicitly defined, should use that value
        let mdl = "INITIAL TIME = 0
~ ~|
FINAL TIME = 100
~ ~|
TIME STEP = 0.5
~ ~|
SAVEPER = 1
~ ~|
x = 1
~ ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        assert_eq!(project.sim_specs.dt, Dt::Dt(0.5));
        assert_eq!(
            project.sim_specs.save_step,
            Some(Dt::Dt(1.0)),
            "save_step should use explicit SAVEPER value"
        );
    }
}
