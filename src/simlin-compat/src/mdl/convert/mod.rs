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

use helpers::{canonical_name, get_equation_name};
pub use types::ConvertError;
use types::{SimSpecsBuilder, SymbolInfo, SyntheticFlow};

use std::collections::{HashMap, HashSet};

use simlin_core::datamodel::{Dimension, Project, SimMethod, Unit};

use crate::mdl::ast::{MdlItem, SubscriptElement};
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
}

impl<'input> ConversionContext<'input> {
    /// Create a new conversion context from MDL source.
    pub fn new(source: &'input str) -> Result<Self, ConvertError> {
        let mut reader = EquationReader::new(source);
        let items: Result<Vec<MdlItem<'input>>, _> = reader.by_ref().collect();
        let items = items?;

        // Parse settings from remaining source (after equations/views)
        let remaining = reader.remaining();
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
    use simlin_core::datamodel::Dt;

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
}
