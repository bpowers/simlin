// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::datamodel::{self, ModuleReference};
use crate::model::{lower_variable, ScopeStage0};
use crate::variable::{parse_var, Variable};

#[cfg(test)]
fn optional_vec(slice: &[&str]) -> Vec<String> {
    slice.iter().map(|id| id.to_string()).collect()
}

#[cfg(test)]
pub(crate) fn x_aux(ident: &str, eqn: &str, units: Option<&str>) -> datamodel::Variable {
    use datamodel::{Aux, Equation, Variable, Visibility};
    Variable::Aux(Aux {
        ident: ident.to_string(),
        equation: Equation::Scalar(eqn.to_string(), None),
        documentation: "".to_string(),
        units: units.map(|s| s.to_owned()),
        gf: None,
        can_be_module_input: false,
        visibility: Visibility::Private,
    })
}

#[cfg(test)]
pub(crate) fn aux(ident: &str, eqn: &str) -> Variable {
    let var = x_aux(ident, eqn, None);
    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let unit_ctx = crate::units::Context::new(&[], &Default::default()).unwrap();
    let var = parse_var(&[], &var, &mut implicit_vars, &unit_ctx, |mi| {
        Ok(Some(mi.clone()))
    });
    assert!(var.equation_errors().is_none());
    assert!(implicit_vars.is_empty());
    let scope = ScopeStage0 {
        models: &Default::default(),
        dimensions: &Default::default(),
    };
    lower_variable(&scope, "main", &var)
}

#[cfg(test)]
pub(crate) fn x_stock(
    ident: &str,
    eqn: &str,
    inflows: &[&str],
    outflows: &[&str],
    units: Option<&str>,
) -> datamodel::Variable {
    use datamodel::{Equation, Stock, Variable, Visibility};
    Variable::Stock(Stock {
        ident: ident.to_string(),
        equation: Equation::Scalar(eqn.to_string(), None),
        documentation: "".to_string(),
        units: units.map(|s| s.to_owned()),
        inflows: optional_vec(inflows),
        outflows: optional_vec(outflows),
        non_negative: false,
        can_be_module_input: false,
        visibility: Visibility::Private,
    })
}

#[cfg(test)]
pub(crate) fn stock(ident: &str, eqn: &str, inflows: &[&str], outflows: &[&str]) -> Variable {
    let var = x_stock(ident, eqn, inflows, outflows, None);
    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let unit_ctx = crate::units::Context::new(&[], &Default::default()).unwrap();
    let var = parse_var(&[], &var, &mut implicit_vars, &unit_ctx, |mi| {
        Ok(Some(mi.clone()))
    });
    assert!(var.equation_errors().is_none());
    assert!(implicit_vars.is_empty());
    let scope = ScopeStage0 {
        models: &Default::default(),
        dimensions: &Default::default(),
    };
    lower_variable(&scope, "main", &var)
}

#[cfg(test)]
pub(crate) fn x_model(ident: &str, variables: Vec<datamodel::Variable>) -> datamodel::Model {
    datamodel::Model {
        name: ident.to_string(),
        variables,
        views: vec![],
    }
}

#[cfg(test)]
pub(crate) fn x_project(
    sim_specs: datamodel::SimSpecs,
    models: &[datamodel::Model],
) -> datamodel::Project {
    datamodel::Project {
        name: "test project".to_owned(),
        sim_specs,
        dimensions: vec![],
        units: vec![],
        models: models.to_vec(),
        source: Default::default(),
    }
}

#[cfg(test)]
pub(crate) fn x_module(
    ident: &str,
    refs: &[(&str, &str)],
    units: Option<&str>,
) -> datamodel::Variable {
    use datamodel::{Module, Variable, Visibility};
    let references: Vec<ModuleReference> = refs
        .iter()
        .map(|(src, dst)| ModuleReference {
            src: src.to_string(),
            dst: dst.to_string(),
        })
        .collect();

    Variable::Module(Module {
        ident: ident.to_string(),
        model_name: ident.to_string(),
        documentation: "".to_string(),
        units: units.map(|s| s.to_owned()),
        references,
        can_be_module_input: false,
        visibility: Visibility::Private,
    })
}

#[cfg(test)]
pub(crate) fn x_flow(ident: &str, eqn: &str, units: Option<&str>) -> datamodel::Variable {
    use datamodel::{Equation, Flow, Variable, Visibility};
    Variable::Flow(Flow {
        ident: ident.to_string(),
        equation: Equation::Scalar(eqn.to_string(), None),
        documentation: "".to_string(),
        units: units.map(|s| s.to_owned()),
        gf: None,
        non_negative: false,
        can_be_module_input: false,
        visibility: Visibility::Private,
    })
}

#[cfg(test)]
pub(crate) fn flow(ident: &str, eqn: &str) -> Variable {
    let var = x_flow(ident, eqn, None);
    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let unit_ctx = crate::units::Context::new(&[], &Default::default()).unwrap();
    let var = parse_var(&[], &var, &mut implicit_vars, &unit_ctx, |mi| {
        Ok(Some(mi.clone()))
    });
    assert!(var.equation_errors().is_none());
    assert!(implicit_vars.is_empty());
    let scope = ScopeStage0 {
        models: &Default::default(),
        dimensions: &Default::default(),
    };
    lower_variable(&scope, "main", &var)
}

#[cfg(test)]
pub(crate) fn sim_specs_with_units(time_units: &str) -> crate::datamodel::SimSpecs {
    crate::datamodel::SimSpecs {
        start: 0.0,
        stop: 0.0,
        dt: Default::default(),
        save_step: None,
        sim_method: Default::default(),
        time_units: Some(time_units.to_owned()),
    }
}
