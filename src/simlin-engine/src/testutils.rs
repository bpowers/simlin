// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::datamodel::{self, ModuleReference};
use crate::model::{ScopeStage0, lower_variable};
use crate::variable::{Variable, parse_var};

#[cfg(test)]
fn optional_vec(slice: &[&str]) -> Vec<String> {
    slice.iter().map(|id| id.to_string()).collect()
}

#[cfg(test)]
pub(crate) fn x_aux(ident: &str, eqn: &str, units: Option<&str>) -> datamodel::Variable {
    use datamodel::{Aux, Equation, Variable};
    Variable::Aux(Aux {
        ident: ident.to_string(),
        equation: Equation::Scalar(eqn.to_string()),
        documentation: "".to_string(),
        units: units.map(|s| s.to_owned()),
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
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
        model_name: "main",
    };
    lower_variable(&scope, &var)
}

#[cfg(test)]
pub(crate) fn x_stock(
    ident: &str,
    eqn: &str,
    inflows: &[&str],
    outflows: &[&str],
    units: Option<&str>,
) -> datamodel::Variable {
    use datamodel::{Equation, Stock, Variable};
    Variable::Stock(Stock {
        ident: ident.to_string(),
        equation: Equation::Scalar(eqn.to_string()),
        documentation: "".to_string(),
        units: units.map(|s| s.to_owned()),
        inflows: optional_vec(inflows),
        outflows: optional_vec(outflows),
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
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
        model_name: "main",
    };
    lower_variable(&scope, &var)
}

#[cfg(test)]
pub(crate) fn x_model(ident: &str, variables: Vec<datamodel::Variable>) -> datamodel::Model {
    datamodel::Model {
        name: ident.to_string(),
        sim_specs: None,
        variables,
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
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
        ai_information: None,
    }
}

#[cfg(test)]
pub(crate) fn x_module(
    ident: &str,
    refs: &[(&str, &str)],
    units: Option<&str>,
) -> datamodel::Variable {
    use datamodel::{Module, Variable};
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
        compat: datamodel::Compat::default(),
        ai_state: None,
        uid: None,
    })
}

#[cfg(test)]
pub(crate) fn x_flow(ident: &str, eqn: &str, units: Option<&str>) -> datamodel::Variable {
    use datamodel::{Equation, Flow, Variable};
    Variable::Flow(Flow {
        ident: ident.to_string(),
        equation: Equation::Scalar(eqn.to_string()),
        documentation: "".to_string(),
        units: units.map(|s| s.to_owned()),
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
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
        model_name: "main",
    };
    lower_variable(&scope, &var)
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

/// A minimal single-stock feedback loop project used across multiple test modules.
/// Contains population (stock), births (flow), and birth_rate (aux) in a positive loop.
#[cfg(test)]
pub(crate) fn feedback_loop_project() -> crate::datamodel::Project {
    crate::datamodel::Project {
        name: "feedback".to_string(),
        sim_specs: crate::datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: crate::datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: crate::datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![crate::datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                crate::datamodel::Variable::Stock(crate::datamodel::Stock {
                    ident: "population".to_string(),
                    equation: crate::datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["births".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: crate::datamodel::Compat::default(),
                }),
                crate::datamodel::Variable::Flow(crate::datamodel::Flow {
                    ident: "births".to_string(),
                    equation: crate::datamodel::Equation::Scalar(
                        "population * birth_rate".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: crate::datamodel::Compat::default(),
                }),
                crate::datamodel::Variable::Aux(crate::datamodel::Aux {
                    ident: "birth_rate".to_string(),
                    equation: crate::datamodel::Equation::Scalar("0.1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: crate::datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}
