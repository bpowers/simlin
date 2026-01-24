// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Variable building methods for MDL to datamodel conversion.

use std::collections::HashMap;

use simlin_core::datamodel::{
    self, DimensionElements, Equation, GraphicalFunction, GraphicalFunctionKind,
    GraphicalFunctionScale, Model, Project, Variable, Visibility,
};

use crate::mdl::ast::{CallKind, Equation as MdlEquation, Expr, FullEquation, Lhs, Subscript};
use crate::mdl::builtins::to_lower_space;
use crate::mdl::xmile_compat::{format_unit_expr, space_to_underbar};

use super::ConversionContext;
use super::helpers::{canonical_name, cartesian_product, format_number, get_lhs};
use super::types::{ConvertError, SymbolInfo, VariableType};

impl<'input> ConversionContext<'input> {
    /// Build the final Project from collected symbols.
    pub(super) fn build_project(self) -> Result<Project, ConvertError> {
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

                // Expand each subscript (element -> single element, dimension -> all elements)
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

        // Build element map: key -> (equation_string, initial_equation, gf)
        // Later equations override earlier ones (element-specific overrides apply-to-all)
        let mut element_map: HashMap<String, (String, Option<String>, Option<GraphicalFunction>)> =
            HashMap::new();

        for exp_eq in expanded_eqs {
            let (eq_str, initial_eq, gf) = self.build_equation_rhs(
                name,
                &exp_eq.eq.equation,
                info.var_type == VariableType::Stock,
            );

            for key in exp_eq.element_keys {
                element_map.insert(key, (eq_str.clone(), initial_eq.clone(), gf.clone()));
            }
        }

        // Convert map to sorted vector
        let mut elements: Vec<(String, String, Option<String>, Option<GraphicalFunction>)> =
            element_map
                .into_iter()
                .map(|(key, (eq_str, initial_eq, gf))| (key, eq_str, initial_eq, gf))
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

    /// Build the equation RHS string, handling stock initial values and ACTIVE INITIAL.
    /// The var_name is used for extrapolation detection on lookups.
    /// Returns (equation, initial_equation, graphical_function).
    fn build_equation_rhs(
        &self,
        var_name: &str,
        eq: &MdlEquation<'_>,
        is_stock: bool,
    ) -> (String, Option<String>, Option<GraphicalFunction>) {
        match eq {
            MdlEquation::Regular(_, expr) => {
                if is_stock && let Some(initial) = self.extract_integ_initial(expr) {
                    return (self.formatter.format_expr(initial), None, None);
                }
                // Check for ACTIVE INITIAL and split into equation + initial_equation
                if let Some((equation_expr, initial_expr)) = self.extract_active_initial(expr) {
                    let eq_str = self.formatter.format_expr(equation_expr);
                    let initial_str = self.formatter.format_expr(initial_expr);
                    return (eq_str, Some(initial_str), None);
                }
                (self.formatter.format_expr(expr), None, None)
            }
            MdlEquation::Lookup(_, table) => {
                // For lookups, return empty string - the GF will be attached
                (
                    String::new(),
                    None,
                    Some(self.build_graphical_function(var_name, table)),
                )
            }
            MdlEquation::WithLookup(_, input, table) => (
                self.formatter.format_expr(input),
                None,
                Some(self.build_graphical_function(var_name, table)),
            ),
            _ => (String::new(), None, None),
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
    /// For ACTIVE INITIAL, split into equation and initial_equation fields.
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

                // Check for ACTIVE INITIAL and split into equation + initial_equation
                if let Some((equation_expr, initial_expr)) = self.extract_active_initial(expr) {
                    let eq_str = self.formatter.format_expr(equation_expr);
                    let initial_str = self.formatter.format_expr(initial_expr);
                    return (
                        self.make_equation_with_initial(lhs, &eq_str, Some(initial_str)),
                        None,
                    );
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
        self.make_equation_with_initial(lhs, eq_str, None)
    }

    /// Create an Equation from LHS, equation string, and optional initial equation.
    fn make_equation_with_initial(
        &self,
        lhs: &Lhs<'_>,
        eq_str: &str,
        initial_str: Option<String>,
    ) -> Equation {
        if lhs.subscripts.is_empty() {
            Equation::Scalar(eq_str.to_string(), initial_str)
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
            Equation::ApplyToAll(dims, eq_str.to_string(), initial_str)
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

    /// Extract the equation and initial expressions from an ACTIVE INITIAL call.
    /// ACTIVE INITIAL(equation, initial) -> (equation, initial)
    fn extract_active_initial<'a>(
        &self,
        expr: &'a Expr<'input>,
    ) -> Option<(&'a Expr<'input>, &'a Expr<'input>)> {
        match expr {
            Expr::App(name, _, args, CallKind::Builtin, _)
                if to_lower_space(name) == "active initial" =>
            {
                if args.len() >= 2 {
                    return Some((&args[0], &args[1]));
                }
                None
            }
            Expr::Paren(inner, _) => self.extract_active_initial(inner),
            _ => None,
        }
    }

    /// Build a GraphicalFunction from a LookupTable.
    /// The var_name is used to check if LOOKUP EXTRAPOLATE was used with this lookup.
    fn build_graphical_function(
        &self,
        var_name: &str,
        table: &crate::mdl::ast::LookupTable,
    ) -> GraphicalFunction {
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

#[cfg(test)]
mod tests {
    use super::super::convert_mdl;
    use simlin_core::datamodel::{Equation, Variable};

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

    #[test]
    fn test_active_initial_scalar() {
        // Scalar ACTIVE INITIAL should have equation and initial_equation fields
        let mdl = "x = ACTIVE INITIAL(y * 2, 100)
~ Units
~ Variable with active initial |
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
                Equation::Scalar(eq, initial_eq) => {
                    assert_eq!(eq, "y * 2", "Equation should be the first argument");
                    assert_eq!(
                        initial_eq.as_deref(),
                        Some("100"),
                        "Initial equation should be the second argument"
                    );
                }
                other => panic!("Expected Scalar equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_active_initial_apply_to_all() {
        // Apply-to-all ACTIVE INITIAL should have equation and initial_equation fields
        let mdl = "DimA: a1, a2, a3
~ ~|
x[DimA] = ACTIVE INITIAL(y[DimA] * 2, 100)
~ Units
~ Array with active initial |
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
                    // All elements should have equation and initial_equation
                    for (key, eq, initial_eq, _) in elements {
                        assert!(
                            ["a1", "a2", "a3"].contains(&key.as_str()),
                            "Unexpected key: {}",
                            key
                        );
                        assert_eq!(eq, "y[DimA] * 2", "Equation should be the first argument");
                        assert_eq!(
                            initial_eq.as_deref(),
                            Some("100"),
                            "Initial equation should be the second argument for {}",
                            key
                        );
                    }
                }
                other => panic!("Expected Arrayed equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_active_initial_element_specific() {
        // Element-specific ACTIVE INITIAL equations
        let mdl = "DimA: a1, a2, a3
~ ~|
x[a1] = ACTIVE INITIAL(y, 10)
~ Units
~ Element specific with active initial |
x[a2] = ACTIVE INITIAL(y * 2, 20)
~ Units
~ Another element |
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
                    assert_eq!(elements.len(), 2, "Should have 2 elements");

                    let a1_eq = elements.iter().find(|(k, _, _, _)| k == "a1");
                    let a2_eq = elements.iter().find(|(k, _, _, _)| k == "a2");

                    assert!(a1_eq.is_some(), "Should have a1");
                    assert!(a2_eq.is_some(), "Should have a2");

                    let (_, a1_expr, a1_init, _) = a1_eq.unwrap();
                    assert_eq!(a1_expr, "y");
                    assert_eq!(a1_init.as_deref(), Some("10"));

                    let (_, a2_expr, a2_init, _) = a2_eq.unwrap();
                    assert_eq!(a2_expr, "y * 2");
                    assert_eq!(a2_init.as_deref(), Some("20"));
                }
                other => panic!("Expected Arrayed equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }
}
