// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{HashMap, HashSet};

use crate::common::{EquationError, EquationResult, Error, ErrorCode, ErrorKind, Ident, Result};
use crate::datamodel::Dimension;
use crate::variable::{parse_var, ModuleInput, Variable};
use crate::{datamodel, eqn_err, model_err};

#[derive(Clone, PartialEq, Debug)]
pub struct Model {
    pub name: String,
    pub variables: HashMap<String, Variable>,
    pub errors: Option<Vec<Error>>,
    pub dt_deps: Option<HashMap<Ident, HashSet<Ident>>>,
    pub initial_deps: Option<HashMap<Ident, HashSet<Ident>>>,
}

// to ensure we sort the list of variables in O(n*log(n)) time, we
// need to iterate over the set of variables we have and compute
// their recursive dependencies.  (assuming this function runs
// in <= O(n*log(n)))
fn all_deps<'a>(vars: &'a [Variable], is_initial: bool) -> Result<HashMap<Ident, HashSet<Ident>>> {
    let mut processing: HashSet<&'a str> = HashSet::new();
    let mut all_vars: HashMap<&'a str, &'a Variable> =
        vars.iter().map(|v| (v.ident(), v)).collect();
    let mut all_var_deps: HashMap<&'a str, Option<HashSet<Ident>>> =
        vars.iter().map(|v| (v.ident(), None)).collect();

    fn all_deps_inner<'a>(
        id: &'a str,
        is_initial: bool,
        processing: &mut HashSet<&'a str>,
        all_vars: &mut HashMap<&'a str, &'a Variable>,
        all_var_deps: &mut HashMap<&'a str, Option<HashSet<Ident>>>,
    ) -> Result<()> {
        let var = all_vars[id];

        // short circuit if we've already figured this out
        if all_var_deps[id].is_some() {
            return Ok(());
        }

        // dependency chains break at stocks, as we use their value from the
        // last dt timestep.  BUT if we are calculating dependencies in the
        // initial dt, then we need to treat stocks as ordinary variables.
        if var.is_stock() && !is_initial {
            all_var_deps.insert(id, Some(HashSet::new()));
            return Ok(());
        }

        processing.insert(id);

        // all deps start out as the direct deps
        let mut all_deps: HashSet<Ident> = HashSet::new();

        for dep in var.direct_deps().iter().map(|d| d.as_str()) {
            // TODO: we could potentially handle this by passing around some context
            //   variable, but its just terrible.
            if dep.starts_with("\\.") {
                return model_err!(NoAbsoluteReferences, id.to_string());
            }

            // if the dependency was e.g. "submodel.output", we only depend on submodel
            let dep = dep.splitn(2, '.').next().unwrap();

            if !all_vars.contains_key(dep) {
                // TODO: this is probably an error
                continue;
            }

            if !all_vars[dep].is_stock() || is_initial {
                all_deps.insert(dep.to_string());
            }

            // ensure we don't blow the stack
            if processing.contains(dep) {
                return model_err!(CircularDependency, id.to_string());
            }

            if all_var_deps[dep].is_none() {
                all_deps_inner(dep, is_initial, processing, all_vars, all_var_deps)?;
            }

            let dep_deps = all_var_deps[dep].as_ref().unwrap();
            all_deps.extend(dep_deps.iter().cloned());
        }

        processing.remove(id);

        all_var_deps.insert(id, Some(all_deps));

        Ok(())
    };

    for var in vars.iter() {
        all_deps_inner(
            var.ident(),
            is_initial,
            &mut processing,
            &mut all_vars,
            &mut all_var_deps,
        )?;
    }

    // this unwrap is safe, because of the full iteration over vars directly above
    let var_deps: HashMap<Ident, HashSet<Ident>> = all_var_deps
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.unwrap()))
        .collect();

    Ok(var_deps)
}

pub fn resolve_relative<'a>(
    models: &HashMap<String, HashMap<Ident, &'a datamodel::Variable>>,
    model_name: &str,
    ident: &str,
) -> Option<&'a datamodel::Variable> {
    let ident = if model_name == "main" && ident.starts_with('.') {
        &ident[1..]
    } else {
        ident
    };
    let model = models.get(model_name)?;

    let input_prefix = format!("{}.", model_name);
    // TODO: this is weird to do here and not before we call into this fn
    let ident = ident.strip_prefix(&input_prefix).unwrap_or(ident);

    // if the identifier is still dotted, its a further submodel reference
    // TODO: this will have to change when we break `module ident == model name`
    if let Some(pos) = ident.find('.') {
        let submodel_name = &ident[..pos];
        let submodel_var = &ident[pos + 1..];
        resolve_relative(models, submodel_name, submodel_var)
    } else {
        Some(model.get(ident)?)
    }
}

pub fn resolve_module_input<'a>(
    models: &HashMap<String, HashMap<Ident, &datamodel::Variable>>,
    model_name: &str,
    ident: &str,
    orig_src: &'a str,
    orig_dst: &'a str,
) -> EquationResult<ModuleInput> {
    use crate::common::canonicalize;
    let input_prefix = format!("{}.", ident);
    let maybe_strip_leading_dot = |s: &'a str| -> &'a str {
        if model_name == "main" && s.starts_with('.') {
            &s[1..]
        } else {
            s
        }
    };
    let src: Ident = canonicalize(maybe_strip_leading_dot(orig_src));
    let dst: Ident = canonicalize(maybe_strip_leading_dot(orig_dst));

    let dst = dst.strip_prefix(&input_prefix);
    if dst.is_none() {
        return eqn_err!(BadModuleInputDst, 0, 0);
    }
    let dst = dst.unwrap().to_string();

    // TODO: reevaluate if this is really the best option here
    // if the source is a temporary created by the engine, assume it is OK
    if (&src).starts_with("$·") {
        return Ok(ModuleInput { src, dst });
    }

    match resolve_relative(models, model_name, &src) {
        Some(_) => Ok(ModuleInput { src, dst }),
        None => eqn_err!(BadModuleInputSrc, 0, 0),
    }
}

impl Model {
    pub fn new(
        models: &HashMap<String, HashMap<Ident, &datamodel::Variable>>,
        x_model: &datamodel::Model,
        dimensions: &[Dimension],
    ) -> Self {
        let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();

        let mut variable_list: Vec<Variable> = x_model
            .variables
            .iter()
            .map(|v| parse_var(models, &x_model.name, dimensions, v, &mut implicit_vars))
            .collect();

        {
            // FIXME: this is an unfortunate API choice
            let mut dummy_implicit_vars: Vec<datamodel::Variable> = Vec::new();
            variable_list.extend(implicit_vars.into_iter().map(|x_var| {
                parse_var(
                    models,
                    &x_model.name,
                    dimensions,
                    &x_var,
                    &mut dummy_implicit_vars,
                )
            }));
            assert_eq!(0, dummy_implicit_vars.len());
        }

        let mut errors: Vec<Error> = Vec::new();

        let dt_deps = match all_deps(&variable_list, false) {
            Ok(deps) => Some(deps),
            Err(err) => {
                errors.push(err);
                None
            }
        };

        let initial_deps = match all_deps(&variable_list, true) {
            Ok(deps) => Some(deps),
            Err(err) => {
                errors.push(err);
                None
            }
        };

        let mut variables: HashMap<String, Variable> = variable_list
            .into_iter()
            .map(|v| (v.ident().to_string(), v))
            .collect();
        let variable_names: HashSet<String> = variables.keys().cloned().collect();

        let mut variables_have_errors = false;
        for (_ident, var) in variables.iter_mut() {
            let mut missing_deps = vec![];
            for dep in var.direct_deps().iter() {
                let dep = if let Some(dot_off) = dep.find('.') {
                    &dep[..dot_off]
                } else {
                    dep.as_str()
                };
                if !variable_names.contains(dep) {
                    missing_deps.push(dep.to_owned());
                }
            }
            for dep in missing_deps.into_iter() {
                let loc = var.ast().unwrap().get_var_loc(&dep).unwrap_or_default();
                var.push_error(EquationError {
                    start: loc.start,
                    end: loc.end,
                    code: ErrorCode::UnknownDependency,
                });
                variables_have_errors = true;
            }
        }

        if variables_have_errors {
            errors.push(Error::new(
                ErrorKind::Model,
                ErrorCode::VariablesHaveErrors,
                None,
            ));
        }

        let maybe_errors = match errors.len() {
            0 => None,
            _ => Some(errors),
        };

        Model {
            name: x_model.name.clone(),
            variables,
            errors: maybe_errors,
            dt_deps,
            initial_deps,
        }
    }

    pub fn get_variable_errors(&self) -> HashMap<Ident, Vec<EquationError>> {
        self.variables
            .iter()
            .filter(|(_, var)| var.errors().is_some())
            .map(|(ident, var)| {
                let errors = var.errors().unwrap();
                (ident.clone(), errors.clone())
            })
            .collect()
    }
}

#[cfg(test)]
fn optional_vec(slice: &[&str]) -> Vec<String> {
    slice.iter().map(|id| id.to_string()).collect()
}

#[cfg(test)]
fn x_module(ident: &str, refs: &[(&str, &str)]) -> datamodel::Variable {
    use datamodel::{Module, ModuleReference, Variable};
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
        units: None,
        references,
    })
}

#[cfg(test)]
fn x_flow(ident: &str, eqn: &str) -> datamodel::Variable {
    use datamodel::{Equation, Flow, Variable};
    Variable::Flow(Flow {
        ident: ident.to_string(),
        equation: Equation::Scalar(eqn.to_string()),
        documentation: "".to_string(),
        units: None,
        gf: None,
        non_negative: false,
    })
}

#[cfg(test)]
fn flow(ident: &str, eqn: &str) -> Variable {
    let var = x_flow(ident, eqn);
    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let var = parse_var(&HashMap::new(), "main", &[], &var, &mut implicit_vars);
    assert!(var.errors().is_none());
    assert!(implicit_vars.is_empty());
    var
}

#[cfg(test)]
fn x_aux(ident: &str, eqn: &str) -> datamodel::Variable {
    use datamodel::{Aux, Equation, Variable};
    Variable::Aux(Aux {
        ident: ident.to_string(),
        equation: Equation::Scalar(eqn.to_string()),
        documentation: "".to_string(),
        units: None,
        gf: None,
    })
}

#[cfg(test)]
fn aux(ident: &str, eqn: &str) -> Variable {
    let var = x_aux(ident, eqn);
    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let var = parse_var(&HashMap::new(), "main", &[], &var, &mut implicit_vars);
    assert!(var.errors().is_none());
    assert!(implicit_vars.is_empty());
    var
}

#[cfg(test)]
fn x_stock(ident: &str, eqn: &str, inflows: &[&str], outflows: &[&str]) -> datamodel::Variable {
    use datamodel::{Equation, Stock, Variable};
    Variable::Stock(Stock {
        ident: ident.to_string(),
        equation: Equation::Scalar(eqn.to_string()),
        documentation: "".to_string(),
        units: None,
        inflows: optional_vec(inflows),
        outflows: optional_vec(outflows),
        non_negative: false,
    })
}

#[cfg(test)]
fn stock(ident: &str, eqn: &str, inflows: &[&str], outflows: &[&str]) -> Variable {
    let var = x_stock(ident, eqn, inflows, outflows);
    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let var = parse_var(&HashMap::new(), "main", &[], &var, &mut implicit_vars);
    assert!(var.errors().is_none());
    assert!(implicit_vars.is_empty());
    var
}

#[cfg(test)]
fn x_model(ident: &str, variables: Vec<datamodel::Variable>) -> datamodel::Model {
    datamodel::Model {
        name: ident.to_string(),
        variables,
        views: vec![],
    }
}

#[test]
fn test_module_dependency() {
    let lynxes_model = x_model(
        "lynxes",
        vec![
            x_aux("init", "5"),
            x_stock("lynxes_stock", "100 * init", &["inflow"], &[]),
            x_flow("inflow", "1"),
        ],
    );
    let hares_model = x_model(
        "hares",
        vec![
            x_aux("lynxes", "0"),
            x_stock("hares_stock", "100", &[], &["outflow"]),
            x_flow("outflow", ".1 * hares_stock"),
        ],
    );
    let main_model = x_model(
        "main",
        vec![
            x_aux("main_init", "7"),
            x_module("lynxes", &[("main_init", "lynxes.init")]),
            x_module("hares", &[("lynxes.lynxes", "hares.lynxes")]),
        ],
    );

    let _models: HashMap<String, &datamodel::Model> = vec![
        ("main".to_string(), &main_model),
        ("lynxes".to_string(), &lynxes_model),
        ("hares".to_string(), &hares_model),
    ]
    .into_iter()
    .collect();
}

#[test]
fn test_module_parse() {
    use crate::variable::ModuleInput;
    let inputs: Vec<ModuleInput> = vec![
        ModuleInput {
            src: "area".to_string(),
            dst: "area".to_string(),
        },
        ModuleInput {
            src: "lynxes.lynxes_stock".to_string(),
            dst: "lynxes".to_string(),
        },
    ];
    let direct_deps = vec!["area".to_string()].into_iter().collect();
    let expected = Variable::Module {
        model_name: "hares".to_string(),
        ident: "hares".to_string(),
        units: None,
        inputs,
        errors: vec![],
        direct_deps,
    };

    let lynxes_model = x_model(
        "lynxes",
        vec![
            x_aux("init", "5"),
            x_stock("lynxes_stock", "100 * init", &["inflow"], &[]),
            x_flow("inflow", "1"),
        ],
    );
    let hares_model = x_model(
        "hares",
        vec![
            x_aux("lynxes", "0"),
            x_stock("hares_stock", "100", &[], &["outflow"]),
            x_flow("outflow", ".1 * hares_stock"),
        ],
    );
    let main_model = x_model(
        "main",
        vec![
            x_aux("area", "time"),
            x_module("lynxes", &[]),
            x_module(
                "hares",
                &[
                    ("area", "hares.area"),
                    ("lynxes.lynxes_stock", "hares.lynxes"),
                ],
            ),
        ],
    );

    let models: HashMap<String, HashMap<Ident, &datamodel::Variable>> = vec![
        ("main".to_string(), &main_model),
        ("lynxes".to_string(), &lynxes_model),
        ("hares".to_string(), &hares_model),
    ]
    .into_iter()
    .map(|(name, m)| build_xvars_map(name, m))
    .collect();

    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let actual = parse_var(
        &models,
        "main",
        &[],
        models["main"]["hares"],
        &mut implicit_vars,
    );
    assert!(actual.errors().is_none());
    assert!(implicit_vars.is_empty());
    assert_eq!(expected, actual);
}

pub fn build_xvars_map(
    name: Ident,
    m: &datamodel::Model,
) -> (Ident, HashMap<Ident, &datamodel::Variable>) {
    (
        name,
        m.variables
            .iter()
            .map(|v| (v.get_ident().to_string(), v))
            .collect(),
    )
}

#[test]
fn test_errors() {
    let main_model = x_model("main", vec![x_aux("aux_3", "unknown_variable * 3.14")]);
    let models: HashMap<String, HashMap<Ident, &datamodel::Variable>> =
        vec![("main".to_string(), &main_model)]
            .into_iter()
            .map(|(name, m)| build_xvars_map(name, m))
            .collect();

    let model = Model::new(&models, &main_model, &[]);

    assert!(model.errors.is_some());
    assert_eq!(
        &Error::new(ErrorKind::Model, ErrorCode::VariablesHaveErrors, None),
        &model.errors.as_ref().unwrap()[0]
    );

    let var_errors = model.get_variable_errors();
    assert_eq!(1, var_errors.len());
    assert!(var_errors.contains_key("aux_3"));
    assert_eq!(1, var_errors["aux_3"].len());
    let err = &var_errors["aux_3"][0];
    assert_eq!(
        &EquationError {
            start: 0,
            end: 16,
            code: ErrorCode::UnknownDependency
        },
        err
    );
}

#[test]
fn test_all_deps() {
    use rand::seq::SliceRandom;
    use rand::thread_rng;
    use std::iter::FromIterator;

    fn verify_all_deps(expected_deps_list: &[(&Variable, &[&str])], is_initial: bool) {
        let expected_deps: HashMap<Ident, HashSet<Ident>> = expected_deps_list
            .iter()
            .map(|(v, deps)| {
                (
                    v.ident().to_string(),
                    HashSet::from_iter(deps.iter().map(|s| s.to_string())),
                )
            })
            .collect();

        let mut all_vars: Vec<Variable> = expected_deps_list
            .iter()
            .map(|(v, _)| (*v).clone())
            .collect();
        let deps = all_deps(&all_vars, is_initial).unwrap();

        if expected_deps != deps {
            let failed_dep_order: Vec<_> = all_vars.iter().map(|v| v.ident()).collect();
            eprintln!("failed order: {:?}", failed_dep_order);
            for (v, expected) in expected_deps_list.iter() {
                eprintln!("{}", v.ident());
                let mut expected: Vec<_> = expected.iter().cloned().collect();
                expected.sort();
                eprintln!("  expected: {:?}", expected);
                let mut actual: Vec<_> = deps[v.ident()].iter().collect();
                actual.sort();
                eprintln!("  actual  : {:?}", actual);
            }
        };
        assert_eq!(expected_deps, deps);

        let mut rng = thread_rng();
        // no matter the order of variables in the list, we should get the same all_deps
        // (even though the order of recursion might change)
        for _ in 0..16 {
            all_vars.shuffle(&mut rng);
            let deps = all_deps(&all_vars, is_initial).unwrap();
            assert_eq!(expected_deps, deps);
        }
    }

    let main_model = x_model(
        "main",
        vec![
            x_module("mod_1", &[("aux_3", "mod_1.input")]),
            x_aux("aux_3", "6"),
            x_flow("inflow", "mod_1.output"),
        ],
    );
    let models: HashMap<String, HashMap<Ident, &datamodel::Variable>> =
        vec![("main".to_string(), &main_model)]
            .into_iter()
            .map(|(name, m)| build_xvars_map(name, m))
            .collect();

    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let mod_1 = parse_var(
        &models,
        "main",
        &[],
        models["main"]["mod_1"],
        &mut implicit_vars,
    );
    assert!(implicit_vars.is_empty());
    let aux_3 = aux("aux_3", "6");
    let inflow = flow("inflow", "mod_1.output");
    let expected_deps_list: Vec<(&Variable, &[&str])> = vec![
        (&inflow, &["mod_1", "aux_3"]),
        (&mod_1, &["aux_3"]),
        (&aux_3, &[]),
    ];

    verify_all_deps(&expected_deps_list, false);

    let aux_used_in_initial = aux("aux_used_in_initial", "7");
    let aux_2 = aux("aux_2", "aux_used_in_initial");
    let aux_3 = aux("aux_3", "aux_2");
    let aux_4 = aux("aux_4", "aux_2");
    let inflow = flow("inflow", "aux_3 + aux_4");
    let outflow = flow("outflow", "stock_1");
    let stock_1 = stock("stock_1", "aux_used_in_initial", &["inflow"], &["outflow"]);
    let expected_deps_list: Vec<(&Variable, &[&str])> = vec![
        (&aux_used_in_initial, &[]),
        (&aux_2, &["aux_used_in_initial"]),
        (&aux_3, &["aux_used_in_initial", "aux_2"]),
        (&aux_4, &["aux_used_in_initial", "aux_2"]),
        (&inflow, &["aux_used_in_initial", "aux_2", "aux_3", "aux_4"]),
        (&outflow, &[]),
        (&stock_1, &[]),
    ];

    verify_all_deps(&expected_deps_list, false);

    // test circular references return an error and don't do something like infinitely
    // recurse
    let aux_a = aux("aux_a", "aux_b");
    let aux_b = aux("aux_b", "aux_a");
    let all_vars = vec![aux_a, aux_b];
    let deps_result = all_deps(&all_vars, false);
    assert!(deps_result.is_err());

    // also self-references should return an error and not blow stock
    let aux_a = aux("aux_a", "aux_a");
    let all_vars = vec![aux_a];
    let deps_result = all_deps(&all_vars, false);
    assert!(deps_result.is_err());

    // test initials
    let expected_deps_list: Vec<(&Variable, &[&str])> = vec![
        (&aux_used_in_initial, &[]),
        (&aux_2, &["aux_used_in_initial"]),
        (&aux_3, &["aux_used_in_initial", "aux_2"]),
        (&aux_4, &["aux_used_in_initial", "aux_2"]),
        (&inflow, &["aux_used_in_initial", "aux_2", "aux_3", "aux_4"]),
        (&outflow, &["stock_1", "aux_used_in_initial"]),
        (&stock_1, &["aux_used_in_initial"]),
    ];

    verify_all_deps(&expected_deps_list, true);

    // test non-existant variables
}
