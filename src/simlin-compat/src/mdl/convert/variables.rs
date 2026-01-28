// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Variable building methods for MDL to datamodel conversion.

use std::collections::HashMap;

use simlin_core::datamodel::{
    self, DimensionElements, Equation, GraphicalFunction, GraphicalFunctionKind,
    GraphicalFunctionScale, Model, ModelGroup, Project, Variable, View, Visibility,
};

use crate::mdl::view;

use crate::mdl::ast::{CallKind, Equation as MdlEquation, Expr, FullEquation, Lhs, Subscript};
use crate::mdl::builtins::{eq_lower_space, to_lower_space};
use crate::mdl::xmile_compat::space_to_underbar;

use super::ConversionContext;
use super::helpers::{canonical_name, cartesian_product, extract_metadata, extract_units, get_lhs};
use super::types::{ConvertError, SymbolInfo, VariableType};
use crate::mdl::xmile_compat::format_number;

impl<'input> ConversionContext<'input> {
    /// Build the final Project from collected symbols.
    pub(super) fn build_project(mut self) -> Result<Project, ConvertError> {
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
        variables.sort_by_cached_key(|a| canonical_name(a.get_ident()));

        // Build groups with unique names
        let groups = self.build_groups();

        // Build views from parsed sketch data
        let views = self.build_views();

        let model = Model {
            name: "main".to_string(),
            sim_specs: None,
            variables,
            views,
            loop_metadata: vec![],
            groups,
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

    /// Build ModelGroup instances from collected group info.
    /// Ensures unique group names that don't conflict with symbol namespace.
    /// Matches xmutil's AdjustGroupNames algorithm (Model.cpp:479-503).
    fn build_groups(&mut self) -> Vec<ModelGroup> {
        if self.groups.is_empty() {
            return vec![];
        }

        // Collect all names in symbol namespace for conflict checking:
        // - Equation variable names
        // - Dimension element names
        let mut namespace: std::collections::HashSet<String> =
            self.symbols.keys().cloned().collect();

        // Add dimension element names to namespace
        for dim in &self.dimensions {
            if let simlin_core::datamodel::DimensionElements::Named(names) = &dim.elements {
                for name in names {
                    namespace.insert(canonical_name(name));
                }
            }
        }

        // First pass: make names unique (xmutil preserves spaces, uses " 1" suffix)
        let mut final_names: Vec<String> = Vec::with_capacity(self.groups.len());
        for group in &self.groups {
            // Preserve the original name (don't convert spaces to underscores)
            let mut name = group.name.clone();

            // Make name unique: check against namespace and earlier groups
            // xmutil uses case-insensitive comparison via ToLowerSpace
            loop {
                let canonical = canonical_name(&name);
                let conflicts_namespace = namespace.contains(&canonical);
                let conflicts_earlier_group =
                    final_names.iter().any(|n| canonical_name(n) == canonical);

                if !conflicts_namespace && !conflicts_earlier_group {
                    break;
                }
                // xmutil appends " 1" (with space), not "_1"
                name = format!("{} 1", name);
            }

            final_names.push(name);
        }

        // Second pass: build ModelGroup instances with parent names
        self.groups
            .iter()
            .enumerate()
            .map(|(i, group)| {
                let parent = group.parent_index.map(|idx| final_names[idx].clone());
                // Members are stored as canonical names, convert to space_to_underbar format
                let members = group.members.iter().map(|m| space_to_underbar(m)).collect();

                ModelGroup {
                    name: final_names[i].clone(),
                    doc: None,
                    parent,
                    members,
                    run_enabled: false,
                }
            })
            .collect()
    }

    /// Build views from parsed sketch data.
    fn build_views(&self) -> Vec<View> {
        if self.views.is_empty() {
            return Vec::new();
        }

        // Build symbol namespace for view title deduplication:
        // includes variable names and dimension names.
        // Groups are NOT included: xmutil's MakeViewNamesUnique uses
        // GetNameSpace()->Find() which only contains symbols/dimensions,
        // not groups (groups are adjusted separately by AdjustGroupNames).
        let mut all_names: std::collections::HashSet<String> =
            self.symbols.keys().cloned().collect();
        for dim in &self.dimensions {
            all_names.insert(to_lower_space(&dim.name));
        }

        view::build_views(self.views.clone(), &self.symbols, &all_names)
    }

    /// Select the appropriate equation from a list, implementing PurgeAFOEq logic.
    ///
    /// This matches xmutil's `PurgeAFOEq` algorithm:
    /// 1. First pass: drop all equations with no expression (EmptyRhs)
    /// 2. Second pass: if multiple equations remain and first is AFO, drop it
    /// 3. Return first remaining equation
    pub(super) fn select_equation<'a>(
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
            lhs_subscripts: Vec<String>, // LHS subscript names (raw, for context building)
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
                if let Some(ref mut existing_dims) = parent_dims {
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
                    // If the raw dimension names differ but normalized names match,
                    // the equations span different subranges of the same parent.
                    // Promote parent_dims to the parent dimensions (but not through
                    // equivalences -- alias dimensions should keep their own name).
                    if *existing_dims != dims {
                        *existing_dims = existing_dims
                            .iter()
                            .map(|d| self.resolve_subrange_to_parent(d))
                            .collect();
                    }
                } else {
                    parent_dims = Some(dims);
                }

                // Collect raw LHS subscript names for element context building
                let lhs_subscripts: Vec<String> = lhs
                    .subscripts
                    .iter()
                    .map(|s| match s {
                        Subscript::Element(n, _) | Subscript::BangElement(n, _) => n.to_string(),
                    })
                    .collect();

                // Compute Cartesian product of expanded elements
                let element_keys = cartesian_product(&expanded_elements);
                expanded_eqs.push(ExpandedEquation {
                    eq,
                    element_keys,
                    lhs_subscripts,
                });
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

        // Per-element substitution is only needed when there are multiple equations
        // (element-specific, multi-subrange, or overrides). For a single apply-to-all
        // equation, all elements get the same expression string with dimension names
        // preserved, matching xmutil's ApplyToAll behavior.
        let needs_substitution = expanded_eqs.len() > 1;

        for exp_eq in expanded_eqs {
            for key in &exp_eq.element_keys {
                // Split element key to get per-dimension element names
                let element_parts: Vec<&str> = key.split(',').collect();
                debug_assert_eq!(
                    element_parts.len(),
                    exp_eq.lhs_subscripts.len(),
                    "element key parts count must match LHS subscript count"
                );

                // Build per-element context for substitution (only when needed)
                let var_canonical = canonical_name(name);
                let ctx = if needs_substitution {
                    self.build_element_context(
                        &var_canonical,
                        &exp_eq.lhs_subscripts,
                        &element_parts,
                    )
                } else {
                    crate::mdl::xmile_compat::ElementContext {
                        lhs_var_canonical: var_canonical,
                        substitutions: HashMap::new(),
                        subrange_mappings: HashMap::new(),
                    }
                };

                let (eq_str, initial_eq, gf) = self.build_equation_rhs_with_context(
                    name,
                    &exp_eq.eq.equation,
                    info.var_type == VariableType::Stock,
                    &ctx,
                );

                element_map.insert(key.clone(), (eq_str, initial_eq, gf));
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

        // Format dimension names -- parent_dims are already normalized to
        // parent dimensions when equations span different subranges
        let formatted_dims: Vec<String> = parent_dims
            .iter()
            .map(|d| self.get_formatted_dimension_name(d))
            .collect();

        let equation = Equation::Arrayed(formatted_dims.clone(), elements);

        // Build the variable
        let ident = space_to_underbar(name);
        let (documentation, units) = extract_metadata(&info.equations);

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

    /// Build the equation RHS string with per-element substitution context.
    /// Dimension references in the equation are substituted with specific element names.
    fn build_equation_rhs_with_context(
        &self,
        var_name: &str,
        eq: &MdlEquation<'_>,
        is_stock: bool,
        ctx: &crate::mdl::xmile_compat::ElementContext,
    ) -> (String, Option<String>, Option<GraphicalFunction>) {
        match eq {
            MdlEquation::Regular(_, expr) => {
                if is_stock && let Some(initial) = self.extract_integ_initial(expr) {
                    return (
                        self.formatter.format_expr_with_context(initial, ctx),
                        None,
                        None,
                    );
                }
                if let Some((equation_expr, initial_expr)) = self.extract_active_initial(expr) {
                    let eq_str = self.formatter.format_expr_with_context(equation_expr, ctx);
                    let initial_str = self.formatter.format_expr_with_context(initial_expr, ctx);
                    return (eq_str, Some(initial_str), None);
                }
                (
                    self.formatter.format_expr_with_context(expr, ctx),
                    None,
                    None,
                )
            }
            MdlEquation::Lookup(_, table) => (
                "0+0".to_string(),
                None,
                Some(self.build_graphical_function(var_name, table)),
            ),
            MdlEquation::WithLookup(_, input, table) => (
                self.formatter.format_expr_with_context(input, ctx),
                None,
                Some(self.build_graphical_function(var_name, table)),
            ),
            MdlEquation::EmptyRhs(_, _) => ("0+0".to_string(), None, None),
            _ => (String::new(), None, None),
        }
    }

    /// Build an ElementContext for per-element equation substitution.
    /// Maps each LHS dimension to the specific element being computed.
    fn build_element_context(
        &self,
        var_canonical: &str,
        lhs_subscripts: &[String],
        element_parts: &[&str],
    ) -> crate::mdl::xmile_compat::ElementContext {
        use crate::mdl::xmile_compat::ElementContext;

        debug_assert_eq!(
            lhs_subscripts.len(),
            element_parts.len(),
            "LHS subscript count must match element parts count"
        );

        let mut substitutions = HashMap::new();

        for (sub_name, elem_name) in lhs_subscripts.iter().zip(element_parts.iter()) {
            let dim_canonical = canonical_name(sub_name);
            // Only add substitution if the subscript is a dimension (not already a
            // specific element). If it's already an element, the equation already
            // references it by name and no substitution is needed.
            if self.dimension_elements.contains_key(&dim_canonical) {
                substitutions.insert(dim_canonical, space_to_underbar(elem_name));
            }
        }

        // Build subrange mappings for dimensions not directly on the LHS
        let subrange_mappings = self.build_subrange_mappings(&substitutions);

        ElementContext {
            lhs_var_canonical: var_canonical.to_string(),
            substitutions,
            subrange_mappings,
        }
    }

    /// Build subrange mappings for dimensions that are not directly on the LHS
    /// but can be resolved positionally through a parent or sibling subrange
    /// that IS on the LHS.
    fn build_subrange_mappings(
        &self,
        substitutions: &HashMap<String, String>,
    ) -> HashMap<String, crate::mdl::xmile_compat::SubrangeMapping> {
        use crate::mdl::xmile_compat::SubrangeMapping;

        let mut mappings = HashMap::new();

        for (dim_canonical, elements) in &self.dimension_elements {
            // Skip dimensions already in substitutions (they're directly on the LHS)
            if substitutions.contains_key(dim_canonical) {
                continue;
            }

            // Skip empty dimensions
            if elements.is_empty() {
                continue;
            }

            // Check if this dimension is a subrange
            let parent_canonical = self.resolve_subrange_to_parent(dim_canonical);
            if parent_canonical == *dim_canonical {
                // Not a subrange
                continue;
            }

            // Case 1: the parent dimension is directly in substitutions
            if substitutions.contains_key(&parent_canonical) {
                if let Some(parent_elements) = self.dimension_elements.get(&parent_canonical) {
                    mappings.insert(
                        dim_canonical.clone(),
                        SubrangeMapping {
                            lhs_dim_canonical: parent_canonical,
                            lhs_dim_elements: parent_elements.clone(),
                            own_elements: elements.clone(),
                        },
                    );
                }
                continue;
            }

            // Case 2: a sibling subrange of the same parent is in substitutions.
            // E.g., LHS is upper (subrange of layers), RHS references lower
            // (also subrange of layers). Map through the sibling.
            for sub_dim in substitutions.keys() {
                let sub_parent = self.resolve_subrange_to_parent(sub_dim);
                if sub_parent == parent_canonical
                    && let Some(sibling_elements) = self.dimension_elements.get(sub_dim.as_str())
                {
                    mappings.insert(
                        dim_canonical.clone(),
                        SubrangeMapping {
                            lhs_dim_canonical: sub_dim.clone(),
                            lhs_dim_elements: sibling_elements.clone(),
                            own_elements: elements.clone(),
                        },
                    );
                    break;
                }
            }
        }

        mappings
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
        let units = extract_units(eq);

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
                (self.make_equation(lhs, "0+0"), Some(gf))
            }
            MdlEquation::WithLookup(lhs, input, table) => {
                let gf = self.build_graphical_function(&lhs.name, table);
                let input_str = self.formatter.format_expr(input);
                (self.make_equation(lhs, &input_str), Some(gf))
            }
            MdlEquation::EmptyRhs(lhs, _) => (self.make_equation(lhs, "0+0"), None),
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
                if eq_lower_space(&dim.name, &canonical_dim)
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
            // Single dimension: just return the elements.
            // len() == 1 guarantees next() returns Some.
            return dim_elements.into_iter().next();
        }

        // Multi-dimensional: compute Cartesian product
        Some(cartesian_product(&dim_elements))
    }

    /// Extract the initial value expression from an INTEG call.
    fn extract_integ_initial<'a>(&self, expr: &'a Expr<'input>) -> Option<&'a Expr<'input>> {
        match expr {
            Expr::App(name, _, args, CallKind::Builtin, _) if eq_lower_space(name, "integ") => {
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
                if eq_lower_space(name, "active initial") =>
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
        // x-scale: use file-specified range if available, otherwise compute from data
        let x_scale = if let Some(x_range) = table.x_range {
            GraphicalFunctionScale {
                min: x_range.0,
                max: x_range.1,
            }
        } else {
            let x_min = x_vals.iter().cloned().fold(f64::INFINITY, f64::min);
            let x_max = x_vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            GraphicalFunctionScale {
                min: x_min,
                max: x_max,
            }
        };

        // y-scale: ALWAYS compute from data points, matching xmutil behavior.
        // xmutil ignores file-specified y-ranges and recomputes from actual data.
        let y_min = y_vals.iter().cloned().fold(f64::INFINITY, f64::min);
        let y_max = y_vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        // When all y-values are identical (ymin == ymax), use ymin+1 as max
        // to avoid degenerate range. Matches xmutil's fallback logic.
        let y_max = if (y_min - y_max).abs() < f64::EPSILON {
            y_min + 1.0
        } else {
            y_max
        };
        let y_scale = GraphicalFunctionScale {
            min: y_min,
            max: y_max,
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
        // Apply-to-all ACTIVE INITIAL: single equation, no per-element substitution.
        // All elements get the same expression with dimension names preserved.
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
                    // Single apply-to-all: all elements have the same expression
                    // with dimension name preserved (no per-element substitution).
                    for (key, eq, initial_eq, _) in elements {
                        assert!(
                            ["a1", "a2", "a3"].contains(&key.as_str()),
                            "Unexpected key: {}",
                            key
                        );
                        assert_eq!(
                            eq, "y[DimA] * 2",
                            "Apply-to-all should preserve dimension name for {}",
                            key
                        );
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

    #[test]
    fn test_range_only_units_produces_dimensionless() {
        let mdl = "x = 5\n~ [0, 100]\n~ Variable with range-only units |\n\\\\\\---///\n";
        let result = convert_mdl(mdl);
        let project = result.unwrap();
        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x");
        if let Some(Variable::Aux(a)) = x {
            assert_eq!(a.units.as_deref(), Some("1"));
        } else {
            panic!("Expected Aux");
        }
    }

    #[test]
    fn test_units_with_expr_and_range_keeps_expr() {
        let mdl = "x = 5\n~ Widgets [0, 100]\n~ Variable with units and range |\n\\\\\\---///\n";
        let result = convert_mdl(mdl);
        let project = result.unwrap();
        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x");
        if let Some(Variable::Aux(a)) = x {
            assert_eq!(a.units.as_deref(), Some("Widgets"));
        } else {
            panic!("Expected Aux");
        }
    }

    #[test]
    fn test_arrayed_variable_range_only_units() {
        let mdl = "DimA: A1, A2 ~~|
x[DimA] = 5
~ [0, 100]
~ Arrayed variable with range-only units |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        let project = result.unwrap();
        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x");
        if let Some(Variable::Aux(a)) = x {
            assert_eq!(a.units.as_deref(), Some("1"));
        } else {
            panic!("Expected Aux");
        }
    }

    #[test]
    fn test_units_from_real_equation_not_afo_placeholder() {
        // When first equation is A FUNCTION OF (no units) and second has units,
        // units should come from the second equation.
        let mdl = "x = A FUNCTION OF(y)
~ ~|
x = y * 2
~ Widgets
~ Real equation with units |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        let project = result.unwrap();
        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x");
        if let Some(Variable::Aux(a)) = x {
            // Units should come from the real equation, not the AFO placeholder
            assert_eq!(a.units.as_deref(), Some("Widgets"));
            // Verify we also got the correct equation
            assert!(matches!(&a.equation, Equation::Scalar(eq, _) if eq == "y * 2"));
        } else {
            panic!("Expected Aux");
        }
    }

    #[test]
    fn test_empty_rhs_scalar_emits_0_plus_0() {
        // An empty RHS (no expression after =) should produce "0+0" to match xmutil
        let mdl = "x =
~ Units
~ Empty equation |
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
                    assert_eq!(eq, "0+0", "Empty RHS should produce '0+0'");
                }
                other => panic!("Expected Scalar equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_empty_rhs_subscripted_emits_0_plus_0() {
        // An empty RHS with subscripts should also produce "0+0"
        let mdl = "DimA: a1, a2
~ ~|
x[DimA] =
~ Units
~ Empty subscripted equation |
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
                Equation::ApplyToAll(dims, eq, _) => {
                    assert_eq!(dims, &["DimA"]);
                    assert_eq!(eq, "0+0", "Empty subscripted RHS should produce '0+0'");
                }
                other => panic!("Expected ApplyToAll equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_units_from_real_equation_when_afo_has_units() {
        // When first equation is A FUNCTION OF WITH units and second also has units,
        // units should come from the second (real) equation since that's what's selected.
        let mdl = "x = A FUNCTION OF(y)
~ OtherUnits
~ AFO with units |
x = y * 2
~ Widgets
~ Real equation with units |
\\\\\\---///
";
        let result = convert_mdl(mdl);
        let project = result.unwrap();
        let x = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "x");
        if let Some(Variable::Aux(a)) = x {
            // Units come from the selected equation (the real one, not AFO)
            assert_eq!(a.units.as_deref(), Some("Widgets"));
        } else {
            panic!("Expected Aux");
        }
    }

    #[test]
    fn test_lookup_only_scalar_emits_0_plus_0() {
        // A scalar variable defined only with a lookup table should have "0+0" as equation
        let mdl = "x( (0,0),(1,1) )
~ ~|
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
                    assert_eq!(eq, "0+0", "Lookup-only variable should have 0+0 equation");
                }
                other => panic!("Expected Scalar equation, got {:?}", other),
            }
            assert!(a.gf.is_some(), "Should have graphical function");
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_lookup_only_arrayed_emits_0_plus_0() {
        // An arrayed variable with element-specific lookups should have "0+0" for each element
        let mdl = "DimA: a1, a2
~ ~|
x[a1]( (0,0),(1,1) )
~ ~|
x[a2]( (0,0),(2,2) )
~ ~|
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
                    assert_eq!(elements.len(), 2);
                    for (_, eq, _, _) in elements {
                        assert_eq!(
                            eq, "0+0",
                            "Lookup-only arrayed element should have 0+0 equation"
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
    fn test_element_specific_metadata_from_last_equation() {
        // In Vensim, only the last element-specific equation carries units and docs.
        // Earlier equations use ~~| (no units, no docs).
        let mdl = "DimA: a1, a2, a3
~ ~|
x[a1] = 1
~ ~|
x[a2] = 2
~ ~|
x[a3] = 3
~ percent/year
~ Annual reduction rate |
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
            assert_eq!(
                a.documentation, "Annual reduction rate",
                "Documentation should come from the last equation"
            );
            assert_eq!(
                a.units.as_deref(),
                Some("percent/year"),
                "Units should come from the last equation"
            );
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_apply_to_all_preserves_dimension_names() {
        // A single apply-to-all equation should NOT substitute dimension names.
        // All elements get the same expression with the dimension name preserved.
        let mdl = "DimA: a1, a2
~ ~|
x[DimA] = y[DimA] * 2
~ Units
~ Apply-to-all |
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
                    assert_eq!(elements.len(), 2);
                    for (_, eq, _, _) in elements {
                        assert_eq!(
                            eq, "y[DimA] * 2",
                            "Apply-to-all should preserve dimension names"
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
    fn test_per_element_dimension_substitution() {
        // When there are multiple equations (element-specific + apply-to-all),
        // per-element substitution kicks in for the apply-to-all equation.
        let mdl = "DimA: a1, a2, a3
~ ~|
x[DimA] = y[DimA] * 2
~ ~|
x[a1] = 10
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

                    let find_eq = |key: &str| -> String {
                        elements
                            .iter()
                            .find(|(k, _, _, _)| k == key)
                            .map(|(_, eq, _, _)| eq.clone())
                            .unwrap_or_else(|| panic!("Should have key {}", key))
                    };

                    // a1 is overridden
                    assert_eq!(find_eq("a1"), "10");
                    // a2 and a3 get substituted from the apply-to-all equation
                    assert_eq!(find_eq("a2"), "y[a2] * 2");
                    assert_eq!(find_eq("a3"), "y[a3] * 2");
                }
                other => panic!("Expected Arrayed equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_per_element_2d_substitution() {
        // 2D per-element substitution: triggered by having multiple equations.
        // Both dimension references are substituted when element-specific
        // overrides are present.
        let mdl = "DimA: a1, a2
~ ~|
DimB: b1, b2
~ ~|
x[DimA, DimB] = y[DimA, DimB] + z[DimA]
~ ~|
x[a1, b1] = 99
~ Units
~ Override |
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
                    assert_eq!(elements.len(), 4);

                    let find_eq = |key: &str| -> String {
                        elements
                            .iter()
                            .find(|(k, _, _, _)| k == key)
                            .map(|(_, eq, _, _)| eq.clone())
                            .unwrap_or_else(|| panic!("Should have key {}", key))
                    };

                    // a1,b1 is overridden
                    assert_eq!(find_eq("a1,b1"), "99");
                    // Other elements get substituted
                    assert_eq!(find_eq("a1,b2"), "y[a1, b2] + z[a1]");
                    assert_eq!(find_eq("a2,b1"), "y[a2, b1] + z[a2]");
                    assert_eq!(find_eq("a2,b2"), "y[a2, b2] + z[a2]");
                }
                other => panic!("Expected Arrayed equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_subrange_positional_resolution() {
        // When equations span multiple subranges, per-element substitution is triggered.
        // The RHS references to sibling subranges are resolved positionally.
        // upper: layer1, layer2, layer3 (subrange of layers)
        // lower: layer2, layer3, layer4 (subrange of layers)
        // bottom: layer4 (subrange of layers)
        // x[upper] = y[upper] - y[lower]  (3 elements)
        // x[bottom] = 0                   (1 element, triggers multi-eq substitution)
        let mdl = "layers: layer1, layer2, layer3, layer4
~ ~|
upper: layer1, layer2, layer3
~ ~|
lower: layer2, layer3, layer4
~ ~|
bottom: layer4
~ ~|
x[upper] = y[upper] - y[lower]
~ ~|
x[bottom] = 0
~ Units
~ Subrange resolution |
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
                    assert_eq!(dims, &["layers"]);
                    assert_eq!(elements.len(), 4);

                    let find_eq = |key: &str| -> String {
                        elements
                            .iter()
                            .find(|(k, _, _, _)| k == key)
                            .map(|(_, eq, _, _)| eq.clone())
                            .unwrap_or_else(|| panic!("Should have key {}", key))
                    };

                    // upper[0]=layer1 -> lower[0]=layer2
                    assert_eq!(find_eq("layer1"), "y[layer1] - y[layer2]");
                    // upper[1]=layer2 -> lower[1]=layer3
                    assert_eq!(find_eq("layer2"), "y[layer2] - y[layer3]");
                    // upper[2]=layer3 -> lower[2]=layer4
                    assert_eq!(find_eq("layer3"), "y[layer3] - y[layer4]");
                    // bottom equation
                    assert_eq!(find_eq("layer4"), "0");
                }
                other => panic!("Expected Arrayed equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_subrange_equations_produce_arrayed() {
        // Variables with equations on different subranges of the same parent dimension
        // should produce an Arrayed equation with the parent dimension name
        let mdl = "layers: layer1, layer2, layer3, layer4
~ ~|
upper: layer1, layer2, layer3
~ ~|
bottom: layer4
~ ~|
x[upper] = 1
~ ~|
x[bottom] = 2
~ ~|
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
                    assert_eq!(
                        dims,
                        &["layers"],
                        "Dimension should be parent 'layers', not subrange names"
                    );
                    assert_eq!(elements.len(), 4, "Should have all 4 layer elements");
                    // layer1, layer2, layer3 from upper (=1), layer4 from bottom (=2)
                    for (key, eq, _, _) in elements {
                        if key == "layer4" {
                            assert_eq!(eq, "2");
                        } else {
                            assert_eq!(eq, "1");
                        }
                    }
                }
                other => panic!("Expected Arrayed equation, got {:?}", other),
            }
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_graphical_function_y_scale_computed_from_data() {
        // y-scale should always be computed from data points, not from file-specified range.
        // This matches xmutil behavior: XMILEGenerator.cpp:513-532 always recomputes y-scale.
        // The file specifies y_range [0,5] but data max is 1.36, so y_scale.max should be 1.36.
        let mdl = "lookup_var(\
            [(0,0)-(2,5)],(0,0.5),(1,1.36),(2,0.8))
~ ~|
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let var = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "lookup_var");
        assert!(var.is_some(), "Should have lookup_var");

        if let Some(Variable::Aux(a)) = var {
            let gf = a.gf.as_ref().expect("Should have graphical function");
            // x-scale uses file range
            assert_eq!(gf.x_scale.min, 0.0);
            assert_eq!(gf.x_scale.max, 2.0);
            // y-scale computed from data, not file range
            assert_eq!(gf.y_scale.min, 0.5);
            assert_eq!(gf.y_scale.max, 1.36);
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_graphical_function_y_scale_all_same_fallback() {
        // When all y-values are identical, ymax should be ymin+1 (degenerate range fallback).
        let mdl = "zero_lookup(\
            [(0,0)-(2,0)],(0,0),(1,0),(2,0))
~ ~|
\\\\\\---///
";
        let result = convert_mdl(mdl);
        assert!(result.is_ok(), "Conversion should succeed: {:?}", result);
        let project = result.unwrap();

        let var = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "zero_lookup");
        assert!(var.is_some(), "Should have zero_lookup");

        if let Some(Variable::Aux(a)) = var {
            let gf = a.gf.as_ref().expect("Should have graphical function");
            assert_eq!(gf.y_scale.min, 0.0);
            assert_eq!(gf.y_scale.max, 1.0); // 0 + 1 = 1
        } else {
            panic!("Expected Aux variable");
        }
    }
}
