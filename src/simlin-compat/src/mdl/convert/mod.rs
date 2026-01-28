// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! MDL AST to datamodel conversion.
//!
//! This module converts parsed MDL AST items directly to `datamodel::Project`,
//! bypassing the XMILE intermediate format.

mod dimensions;
mod helpers;
mod stocks;
mod types;
mod variables;

use crate::mdl::builtins::eq_lower_space;
use helpers::{canonical_name, get_equation_name};
pub use types::ConvertError;
use types::{SimSpecsBuilder, SyntheticFlow};
pub use types::{SymbolInfo, VariableType};

use crate::mdl::view::{self, VensimView};

use std::collections::{HashMap, HashSet};

use simlin_core::datamodel::{Dimension, Project, SimMethod, Unit};

use crate::mdl::ast::{Equation as MdlEquation, MdlItem, SubscriptElement};

/// Information about a group collected during parsing.
struct GroupInfo {
    /// Group name (raw, will be normalized later)
    name: String,
    /// Index of parent group in the groups vec, if any
    parent_index: Option<usize>,
    /// Canonical names of variables in this group
    members: Vec<String>,
}
use crate::mdl::reader::EquationReader;
use crate::mdl::settings::PostEquationParser;
use crate::mdl::xmile_compat::XmileFormatter;

/// Context for MDL to datamodel conversion.
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
    #[allow(dead_code)]
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
    /// Collected groups during parsing
    groups: Vec<GroupInfo>,
    /// Index of current group for variable assignment (None = no group yet)
    current_group_index: Option<usize>,
    /// Parsed views from the sketch section
    views: Vec<VensimView>,
}

impl<'input> ConversionContext<'input> {
    /// Create a new conversion context from MDL source.
    pub fn new(source: &'input str) -> Result<Self, ConvertError> {
        let mut reader = EquationReader::new(source);
        let items: Result<Vec<MdlItem<'input>>, _> = reader.by_ref().collect();
        let items = items?;

        // Parse views and settings from remaining source (after equations)
        let remaining = reader.remaining();

        // Parse views from sketch section
        let views = view::parse_views(remaining)?;

        // Parse settings (after views end marker)
        let settings_parser = PostEquationParser::new(remaining);
        let settings = settings_parser.parse_settings();

        Ok(ConversionContext {
            items,
            symbols: HashMap::new(),
            dimensions: Vec::new(),
            equivalences: HashMap::new(),
            sim_specs: SimSpecsBuilder {
                sim_method: Some(settings.integration_method),
                ..SimSpecsBuilder::default()
            },
            integration_method: settings.integration_method,
            unit_equivs: settings.unit_equivs,
            formatter: XmileFormatter::new(),
            synthetic_flows: Vec::new(),
            element_owners: HashMap::new(),
            dimension_elements: HashMap::new(),
            extrapolate_lookups: HashSet::new(),
            raw_subscript_defs: HashMap::new(),
            groups: Vec::new(),
            current_group_index: None,
            views,
        })
    }

    /// Convert the MDL to a Project.
    pub fn convert(mut self) -> Result<Project, ConvertError> {
        // Pass 1: Collect symbols and build initial symbol table
        self.collect_symbols();

        // Pass 2: Build dimensions from subscript definitions
        self.build_dimensions()?;

        // Pass 2.5: Set subrange dimensions on formatter for bang-subscript formatting
        // Subranges are dimensions with maps_to set (they map to a parent dimension).
        // Use canonical form (to_lower_space) since the formatter lookup also uses
        // to_lower_space for consistency.
        let mut subrange_dims: HashSet<String> = self
            .dimensions
            .iter()
            .filter(|d| d.maps_to.is_some())
            .map(|d| canonical_name(&d.name))
            .collect();

        // Also detect implicit subranges: dimensions whose elements are all owned
        // by a single different parent dimension. These are subranges for bang
        // formatting purposes even without explicit `->` mapping syntax.
        for (dim_canonical, elements) in &self.dimension_elements {
            if subrange_dims.contains(dim_canonical) || elements.is_empty() {
                continue;
            }
            if let Some(first_owner) = elements
                .first()
                .and_then(|e| self.element_owners.get(&canonical_name(e)))
                && first_owner != dim_canonical
                && elements.iter().all(|e| {
                    self.element_owners
                        .get(&canonical_name(e))
                        .map(|o| o == first_owner)
                        .unwrap_or(false)
                })
            {
                subrange_dims.insert(dim_canonical.clone());
            }
        }

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
        // Clone items to avoid borrow issues
        let items = std::mem::take(&mut self.items);

        for item in &items {
            match item {
                MdlItem::Equation(eq) => {
                    if let Some(name) = get_equation_name(&eq.equation) {
                        let canonical = canonical_name(&name);

                        // Assign to group only if:
                        // 1. There's a current group
                        // 2. This is the variable's first equation
                        // 3. This is NOT a subscript definition, equivalence, or control variable
                        // Note: Macro equations are self-contained in MacroDef.equations
                        // and never appear in the main items iteration, so we don't
                        // need special handling for them.
                        let is_first_equation = !self.symbols.contains_key(&canonical);
                        let is_variable_equation = Self::is_variable_equation(&eq.equation);
                        if let Some(group_idx) = self.current_group_index
                            && is_first_equation
                            && is_variable_equation
                        {
                            self.groups[group_idx].members.push(canonical.clone());
                        }

                        let info = self
                            .symbols
                            .entry(canonical)
                            .or_insert_with(SymbolInfo::new);
                        info.equations.push((**eq).clone());
                    }
                }
                MdlItem::Group(group) => {
                    // Determine parent using xmutil algorithm (VensimParse.cpp:259-275):
                    // If groups empty OR (first char differs from last group AND new first char is digit):
                    //   Search for existing group with same name as owner
                    // Otherwise: owner is previous group
                    let parent_index = self.determine_group_parent(&group.name);

                    let group_info = GroupInfo {
                        name: group.name.to_string(),
                        parent_index,
                        members: Vec::new(),
                    };
                    self.groups.push(group_info);
                    self.current_group_index = Some(self.groups.len() - 1);
                }
                MdlItem::Macro(_) | MdlItem::EqEnd(_) => {}
            }
        }

        // Restore items
        self.items = items;
    }

    /// Check if an equation represents an actual variable (not a subscript def,
    /// equivalence, or control variable like INITIAL TIME).
    fn is_variable_equation(eq: &MdlEquation<'_>) -> bool {
        // Skip subscript definitions and equivalences
        if matches!(
            eq,
            MdlEquation::SubscriptDef(_, _) | MdlEquation::Equivalence(_, _, _)
        ) {
            return false;
        }

        // Skip control variables
        if let Some(name) = get_equation_name(eq)
            && (eq_lower_space(&name, "initial time")
                || eq_lower_space(&name, "final time")
                || eq_lower_space(&name, "time step")
                || eq_lower_space(&name, "saveper"))
        {
            return false;
        }

        true
    }

    /// Determine the parent index for a new group using xmutil's algorithm.
    /// (VensimParse.cpp:259-275)
    fn determine_group_parent(&self, new_group_name: &str) -> Option<usize> {
        if self.groups.is_empty() {
            return None;
        }

        let new_first_char = new_group_name.chars().next();
        let last_group_first_char = self.groups.last().and_then(|g| g.name.chars().next());

        // If first char differs AND new first char is a digit:
        // Try to find existing group with same name as owner
        let try_find_owner = matches!(
            (new_first_char, last_group_first_char),
            (Some(c1), Some(c2)) if c1.is_ascii_digit() && c1 != c2
        );

        if try_find_owner {
            // Search for existing group with this name as owner
            // (This is what xmutil does, though it usually fails to find a match)
            for (idx, g) in self.groups.iter().enumerate() {
                if g.name == new_group_name {
                    return Some(idx);
                }
            }
        }

        // Default: owner is the previous group
        Some(self.groups.len() - 1)
    }
}

// build_project and other variable building methods are in variables.rs

/// Convert MDL source to a Project.
pub fn convert_mdl(source: &str) -> Result<Project, ConvertError> {
    let ctx = ConversionContext::new(source)?;
    ctx.convert()
}

#[cfg(test)]
mod tests {
    use super::*;
    use simlin_core::datamodel::{Dt, Equation, Variable};

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

    #[test]
    fn test_integration_method_from_settings() {
        // MDL with settings section containing type 15 (integration method)
        let mdl = "x = 1
~ ~|
\\\\\\---///
V300
*View 1
///---\\\\\\
:L<%^E!@
15:0,0,0,1,0,0
";
        let project = convert_mdl(mdl).unwrap();

        assert_eq!(
            project.sim_specs.sim_method,
            SimMethod::RungeKutta4,
            "sim_method should be RK4 from type 15 settings"
        );
    }

    #[test]
    fn test_integration_method_default_euler() {
        // MDL without settings section should default to Euler
        let mdl = "x = 1
~ ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();

        assert_eq!(
            project.sim_specs.sim_method,
            SimMethod::Euler,
            "sim_method should default to Euler"
        );
    }

    #[test]
    fn test_unit_equivalences_from_settings() {
        // MDL with settings section containing type 22 (unit equivalence)
        let mdl = "x = 1
~ Dollar ~|
\\\\\\---///
V300
///---\\\\\\
:L<%^E!@
22:$,Dollar,Dollars,$s
22:Hour,Hours,Hr
";
        let project = convert_mdl(mdl).unwrap();

        assert_eq!(project.units.len(), 2);

        let dollar = &project.units[0];
        assert_eq!(dollar.name, "Dollar");
        assert_eq!(dollar.equation, Some("$".to_string()));
        assert_eq!(dollar.aliases, vec!["Dollars", "$s"]);
        assert!(!dollar.disabled);

        let hour = &project.units[1];
        assert_eq!(hour.name, "Hour");
        assert_eq!(hour.equation, None);
        assert_eq!(hour.aliases, vec!["Hours", "Hr"]);
    }

    #[test]
    fn test_full_mdl_with_settings() {
        // Full MDL file similar to test_control_vars.mdl
        let mdl = r#"x = 5
~ Units
~ A constant |
INITIAL TIME = 0
~ Month ~|
FINAL TIME = 100
~ Month ~|
TIME STEP = 0.25
~ Month ~|
\\\---/// Sketch information
V300
*View 1
$192-192-192,0,Times
10,1,x,100,100,12,11,8,3,0,0,0,0,0,0
///---\\\
:L<%^E!@
1:Current.vdf
15:0,0,0,5,0,0
22:$,Dollar,Dollars
"#;
        let project = convert_mdl(mdl).unwrap();

        // Verify sim_specs
        assert_eq!(project.sim_specs.start, 0.0);
        assert_eq!(project.sim_specs.stop, 100.0);
        assert_eq!(project.sim_specs.dt, Dt::Dt(0.25));
        assert_eq!(
            project.sim_specs.sim_method,
            SimMethod::RungeKutta4,
            "Method code 5 should map to RK4"
        );

        // Verify unit equivalences
        assert_eq!(project.units.len(), 1);
        assert_eq!(project.units[0].name, "Dollar");
    }

    #[test]
    fn test_mdl_group_basic() {
        // Single group with variables using curly brace format
        let mdl = "{**Control**}\nx = 5\n~ Units\n~ Variable in Control group |\n\ny = 10\n~ Units\n~ Another variable |\n\\\\\\---///\n";
        let project = convert_mdl(mdl).unwrap();
        let model = &project.models[0];

        assert_eq!(model.groups.len(), 1);
        assert_eq!(model.groups[0].name, "Control");
        assert!(model.groups[0].parent.is_none());
        // Both x and y should be members
        assert!(model.groups[0].members.contains(&"x".to_string()));
        assert!(model.groups[0].members.contains(&"y".to_string()));
    }

    #[test]
    fn test_mdl_group_multiple() {
        // Multiple groups using curly brace format
        // Group names preserve spaces (xmutil behavior)
        let mdl =
            "{**First Group**}\nx = 5\n~ ~|\n\n{**Second Group**}\ny = 10\n~ ~|\n\\\\\\---///\n";
        let project = convert_mdl(mdl).unwrap();
        let model = &project.models[0];

        assert_eq!(model.groups.len(), 2);
        assert_eq!(model.groups[0].name, "First Group");
        assert_eq!(model.groups[1].name, "Second Group");
        assert!(model.groups[0].members.contains(&"x".to_string()));
        assert!(model.groups[1].members.contains(&"y".to_string()));
    }

    #[test]
    fn test_mdl_group_hierarchy() {
        // Groups with hierarchy: second group's parent is first
        // Group names preserve spaces (xmutil behavior)
        let mdl = "{**Top Level**}\na = 1\n~ ~|\n\n{**Sub Group**}\nb = 2\n~ ~|\n\\\\\\---///\n";
        let project = convert_mdl(mdl).unwrap();
        let model = &project.models[0];

        assert_eq!(model.groups.len(), 2);
        // First group has no parent
        assert!(model.groups[0].parent.is_none());
        // Second group's parent is the first (with preserved spaces)
        assert_eq!(model.groups[1].parent, Some("Top Level".to_string()));
    }

    #[test]
    fn test_mdl_group_first_equation_only() {
        // Variable should be assigned to group on first equation only
        let mdl = "{**Group A**}\nx = A FUNCTION OF(y)\n~ ~|\n\n{**Group B**}\nx = 5\n~ ~|\n\\\\\\---///\n";
        let project = convert_mdl(mdl).unwrap();
        let model = &project.models[0];

        assert_eq!(model.groups.len(), 2);
        // x should be in Group A (first equation), not Group B
        assert!(
            model.groups[0].members.contains(&"x".to_string()),
            "x should be in Group A"
        );
        assert!(
            !model.groups[1].members.contains(&"x".to_string()),
            "x should NOT be in Group B"
        );
    }

    #[test]
    fn test_mdl_group_before_equations() {
        // Variables before any group marker are not assigned to any group
        let mdl = "x = 5\n~ ~|\n\n{**Group A**}\ny = 10\n~ ~|\n\\\\\\---///\n";
        let project = convert_mdl(mdl).unwrap();
        let model = &project.models[0];

        assert_eq!(model.groups.len(), 1);
        // x was defined before any group, should not be in any group
        assert!(
            !model.groups[0].members.contains(&"x".to_string()),
            "x should NOT be in any group"
        );
        // y was defined after group marker
        assert!(
            model.groups[0].members.contains(&"y".to_string()),
            "y should be in Group A"
        );
    }

    #[test]
    fn test_mdl_group_star_format() {
        // Test ***\nname\n***| format (name must not contain spaces in star format)
        let mdl = "***\nMyGroup\n***|\nx = 5\n~ ~|\n\\\\\\---///\n";
        let project = convert_mdl(mdl).unwrap();
        let model = &project.models[0];

        assert_eq!(model.groups.len(), 1);
        assert_eq!(model.groups[0].name, "MyGroup");
        assert!(model.groups[0].members.contains(&"x".to_string()));
    }

    #[test]
    fn test_mdl_group_name_uniqueness() {
        // If a group name conflicts with a symbol, it should be made unique
        // xmutil uses " 1" suffix (with space), not "_1"
        let mdl = "x: a, b\n~ ~|\n\n{**x**}\ny = 5\n~ ~|\n\\\\\\---///\n";
        let project = convert_mdl(mdl).unwrap();
        let model = &project.models[0];

        assert_eq!(model.groups.len(), 1);
        // Group name "x" conflicts with dimension "x", should be "x 1" (with space)
        assert_eq!(model.groups[0].name, "x 1");
    }

    #[test]
    fn test_mdl_group_variables_after_macro() {
        // Variables defined after a macro should still be assigned to their group.
        // This tests that macro handling doesn't break group membership.
        let mdl = "{**Control**}\nx = 5\n~ ~|\n\n:MACRO: MYMACRO(input)\nmacro_var = input * 2\n~ ~|\n:END OF MACRO:\n\ny = 10\n~ ~|\n\\\\\\---///\n";
        let project = convert_mdl(mdl).unwrap();
        let model = &project.models[0];

        assert_eq!(model.groups.len(), 1);
        assert_eq!(model.groups[0].name, "Control");
        // Both x and y should be in the Control group
        assert!(
            model.groups[0].members.contains(&"x".to_string()),
            "x should be in Control group"
        );
        assert!(
            model.groups[0].members.contains(&"y".to_string()),
            "y should be in Control group (after macro)"
        );
        // macro_var should NOT be in the group (it's inside a macro)
        assert!(
            !model.groups[0].members.contains(&"macro_var".to_string()),
            "macro_var should NOT be in any group (inside macro)"
        );
    }

    #[test]
    fn test_mdl_to_xmile_groups_roundtrip() {
        // Test that MDL groups survive conversion through XMILE
        use crate::xmile::{project_from_reader, project_to_xmile};
        use std::io::Cursor;

        let mdl = "{**Control**}\nx = 5\n~ ~|\n\n{**Settings**}\ny = 10\n~ ~|\n\\\\\\---///\n";
        let project = convert_mdl(mdl).unwrap();
        let model = &project.models[0];

        // Verify initial MDL conversion has groups
        assert_eq!(model.groups.len(), 2);
        assert_eq!(model.groups[0].name, "Control");
        assert_eq!(model.groups[1].name, "Settings");
        assert!(model.groups[0].members.contains(&"x".to_string()));
        assert!(model.groups[1].members.contains(&"y".to_string()));

        // Convert to XMILE and back
        let xmile_str = project_to_xmile(&project).unwrap();
        let mut cursor = Cursor::new(xmile_str.as_bytes());
        let roundtripped = project_from_reader(&mut cursor).unwrap();
        let roundtripped_model = &roundtripped.models[0];

        // Verify groups survived the roundtrip
        assert_eq!(roundtripped_model.groups.len(), 2);
        assert_eq!(roundtripped_model.groups[0].name, "Control");
        assert_eq!(roundtripped_model.groups[1].name, "Settings");
        assert!(
            roundtripped_model.groups[0]
                .members
                .contains(&"x".to_string())
        );
        assert!(
            roundtripped_model.groups[1]
                .members
                .contains(&"y".to_string())
        );
    }

    #[test]
    fn test_mdl_group_excludes_subscript_definitions() {
        // P2: Subscript definitions should NOT be added to group members
        let mdl = "{**Control**}\nDim: a, b, c\n~ ~|\nx = 5\n~ ~|\n\\\\\\---///\n";
        let project = convert_mdl(mdl).unwrap();
        let model = &project.models[0];

        assert_eq!(model.groups.len(), 1);
        // x should be in the group
        assert!(
            model.groups[0].members.contains(&"x".to_string()),
            "x should be in Control group"
        );
        // Dim (subscript definition) should NOT be in the group (check both cases)
        assert!(
            !model.groups[0].members.contains(&"Dim".to_string())
                && !model.groups[0].members.contains(&"dim".to_string()),
            "Subscript definition 'Dim' should NOT be in group members, got: {:?}",
            model.groups[0].members
        );
    }

    #[test]
    fn test_mdl_group_excludes_control_variables() {
        // P2: Control variables (INITIAL TIME, FINAL TIME, etc.) should NOT be in group members
        let mdl = "{**Control**}\nINITIAL TIME = 0\n~ ~|\nFINAL TIME = 100\n~ ~|\nTIME STEP = 1\n~ ~|\nx = 5\n~ ~|\n\\\\\\---///\n";
        let project = convert_mdl(mdl).unwrap();
        let model = &project.models[0];

        assert_eq!(model.groups.len(), 1);

        // Debug: print members
        eprintln!("Group members: {:?}", model.groups[0].members);

        // x should be in the group
        assert!(
            model.groups[0].members.contains(&"x".to_string()),
            "x should be in Control group"
        );
        // Control variables should NOT be in the group
        // Members are stored with space_to_underbar format
        assert!(
            !model.groups[0]
                .members
                .contains(&"INITIAL_TIME".to_string())
                && !model.groups[0]
                    .members
                    .contains(&"initial_time".to_string()),
            "INITIAL TIME should NOT be in group members"
        );
        assert!(
            !model.groups[0].members.contains(&"FINAL_TIME".to_string())
                && !model.groups[0].members.contains(&"final_time".to_string()),
            "FINAL TIME should NOT be in group members"
        );
        assert!(
            !model.groups[0].members.contains(&"TIME_STEP".to_string())
                && !model.groups[0].members.contains(&"time_step".to_string()),
            "TIME STEP should NOT be in group members"
        );
    }

    #[test]
    fn test_mdl_group_name_preserves_spaces() {
        // Group names should preserve spaces, not convert to underscores
        let mdl = "{**Control Panel**}\nx = 5\n~ ~|\n\\\\\\---///\n";
        let project = convert_mdl(mdl).unwrap();
        let model = &project.models[0];

        assert_eq!(model.groups.len(), 1);
        assert_eq!(
            model.groups[0].name, "Control Panel",
            "Group name should preserve spaces"
        );
    }

    #[test]
    fn test_mdl_group_name_conflict_uses_space_suffix() {
        // When group name conflicts, xmutil appends " 1" (with space), not "_1"
        let mdl = "x = 1\n~ ~|\n{**x**}\ny = 5\n~ ~|\n\\\\\\---///\n";
        let project = convert_mdl(mdl).unwrap();
        let model = &project.models[0];

        assert_eq!(model.groups.len(), 1);
        // Group name "x" conflicts with variable "x", should be "x 1" (with space)
        assert_eq!(
            model.groups[0].name, "x 1",
            "Conflicting group name should use ' 1' suffix (with space)"
        );
    }

    #[test]
    fn test_mdl_group_name_conflict_with_dimension_element() {
        // Group name conflict detection should include dimension elements
        let mdl = "Dim: elem1, elem2\n~ ~|\n{**elem1**}\nx = 5\n~ ~|\n\\\\\\---///\n";
        let project = convert_mdl(mdl).unwrap();
        let model = &project.models[0];

        assert_eq!(model.groups.len(), 1);
        // Group name "elem1" conflicts with dimension element "elem1"
        assert_eq!(
            model.groups[0].name, "elem1 1",
            "Group name conflicting with dimension element should use ' 1' suffix"
        );
    }

    #[test]
    fn test_implicit_subrange_bang_subscript_formatting() {
        // When a dimension is an implicit subrange (its elements are a subset of
        // a larger dimension, without explicit `->` mapping), BangElement on it
        // should produce "dim.*" not just "*".
        let mdl = "COP: OECD US, OECD EU, DevA, DevB
~ ~|
COP Developed: OECD US, OECD EU
~ ~|
x[COP] = 1
~ ~|
y = SUM(x[COP Developed!])
~ ~|
\\\\\\---///
";
        let project = convert_mdl(mdl).unwrap();
        let y = project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "y")
            .expect("Should have y variable");

        if let Variable::Aux(a) = y {
            let eq = match &a.equation {
                Equation::Scalar(expr, _) => expr.clone(),
                other => panic!("Expected Scalar equation, got {:?}", other),
            };
            assert!(
                eq.contains("COP_Developed.*"),
                "Bang subscript on implicit subrange should produce 'COP_Developed.*', got: {}",
                eq
            );
        } else {
            panic!("Expected Aux variable");
        }
    }

    #[test]
    fn test_mdl_group_numeric_leading_owner_logic() {
        // xmutil's numeric-leading owner logic:
        // If new group starts with digit AND differs from previous group's first char,
        // search for existing group with same name as owner
        let mdl = "{**1 Main**}\na = 1\n~ ~|\n{**1.1 Sub**}\nb = 2\n~ ~|\n{**2 Other**}\nc = 3\n~ ~|\n\\\\\\---///\n";
        let project = convert_mdl(mdl).unwrap();
        let model = &project.models[0];

        assert_eq!(model.groups.len(), 3);
        // "1 Main" has no parent (first group)
        assert!(model.groups[0].parent.is_none());
        // "1.1 Sub" starts with '1', same as previous, so parent is previous ("1 Main")
        assert_eq!(model.groups[1].parent, Some("1 Main".to_string()));
        // "2 Other" starts with '2', differs from '1', and is a digit
        // So it searches for existing group named "2 Other" (not found), then uses previous
        assert_eq!(model.groups[2].parent, Some("1-1 Sub".to_string()));
    }

    #[test]
    fn test_mdl_views_parsed() {
        // Test that views are parsed from the sketch section
        let mdl = r#"x = 5
~ Units
~ A constant |
\\\---/// Sketch information
V300  Do not put anything below this section
*View 1
$192-192-192,0,Helvetica|10|B|0-0-0|0-0-0|0-0-0|-1--1--1|-1--1--1|96,96,100,0
10,1,x,100,200,40,20,3,3,0,0,0,0,0,0
///---\\\
"#;
        let project = convert_mdl(mdl).unwrap();
        let model = &project.models[0];

        // Verify views are populated
        assert_eq!(model.views.len(), 1);

        // Verify the view contains elements
        let simlin_core::datamodel::View::StockFlow(sf) = &model.views[0];
        assert!(!sf.elements.is_empty(), "View should have elements");
    }

    #[test]
    fn test_mdl_views_with_flow() {
        // Test view parsing with stocks, flows, and connectors
        let mdl = r#"Stock = INTEG(inflow, 100)
~ Units
~ Stock |
inflow = 10
~ Units/Time
~ Flow |
\\\---/// Sketch
V300
*Test
$font
10,1,Stock,100,100,40,20,3,3,0,0,0,0,0,0
11,2,444,200,100,6,8,34,3,0,0,1,0,0,0
10,3,inflow,200,120,40,20,40,3,0,0,-1,0,0,0
1,4,2,1,4,0,0,22,0,0,0,-1--1--1,,1|(150,100)|
///---\\\
"#;
        let project = convert_mdl(mdl).unwrap();
        let model = &project.models[0];

        assert_eq!(model.views.len(), 1);
        let simlin_core::datamodel::View::StockFlow(sf) = &model.views[0];
        // Should have at least stock, flow, and maybe link elements
        assert!(
            sf.elements.len() >= 2,
            "View should have stock and flow elements"
        );
    }

    #[test]
    fn test_mdl_no_views_still_works() {
        // Test that MDL files without views still parse correctly
        let mdl = "x = 5\n~ ~|\n\\\\\\---///\n";
        let project = convert_mdl(mdl).unwrap();
        let model = &project.models[0];

        // Views should be empty
        assert!(model.views.is_empty());

        // But the model should still have variables
        assert!(!model.variables.is_empty());
    }
}
