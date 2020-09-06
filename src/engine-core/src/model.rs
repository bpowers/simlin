// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{HashMap, HashSet};

use crate::common::{Error, Ident, Result};
use crate::variable::{parse_var, Variable};
use crate::xmile;

#[derive(Debug)]
pub struct Model {
    pub name: String,
    pub variables: HashMap<String, Variable>,
    pub errors: Option<Vec<Error>>,
    pub dt_deps: Option<HashMap<Ident, HashSet<Ident>>>,
    pub initial_deps: Option<HashMap<Ident, HashSet<Ident>>>,
}

const EMPTY_VARS: xmile::Variables = xmile::Variables {
    variables: Vec::new(),
};

// to ensure we sort the list of variables in O(n*log(n)) time, we
// need to iterate over the set of variables we have and compute
// their recursive dependencies.  (assuming this function runs
// in <= O(n*log(n)))
fn all_deps<'a>(vars: &'a [Variable], is_initial: bool) -> Result<HashMap<Ident, HashSet<Ident>>> {
    let mut processing: HashSet<&'a str> = HashSet::new();
    let mut all_vars: HashMap<&'a str, &'a Variable> =
        vars.iter().map(|v| (v.ident().as_str(), v)).collect();
    let mut all_var_deps: HashMap<&'a str, Option<HashSet<Ident>>> =
        vars.iter().map(|v| (v.ident().as_str(), None)).collect();

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

impl Model {
    pub fn new(x_model: &xmile::Model) -> Self {
        let variable_list: Vec<Variable> = x_model
            .variables
            .as_ref()
            .unwrap_or(&EMPTY_VARS)
            .variables
            .iter()
            .map(parse_var)
            .collect();

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

        let maybe_errors = match errors.len() {
            0 => None,
            _ => Some(errors),
        };

        Model {
            name: x_model.name.as_ref().unwrap_or(&"main".to_string()).clone(),
            variables: variable_list
                .into_iter()
                .map(|v| (v.ident().clone(), v))
                .collect(),
            errors: maybe_errors,
            dt_deps,
            initial_deps,
        }
    }
}

fn optional_vec(slice: &[&str]) -> Option<Vec<String>> {
    if slice.is_empty() {
        None
    } else {
        Some(slice.iter().map(|id| id.to_string()).collect())
    }
}

fn module(ident: &str, refs: &[(&str, &str)]) -> Variable {
    use xmile::{Module, Ref, Var};
    let refs: Option<Vec<Ref>> = if refs.is_empty() {
        None
    } else {
        Some(
            refs.iter()
                .map(|(src, dst)| Ref {
                    src: src.to_string(),
                    dst: dst.to_string(),
                })
                .collect(),
        )
    };

    let x_module = Var::Module(Module {
        name: ident.to_string(),
        doc: None,
        units: None,
        refs,
    });

    let var = parse_var(&x_module);
    assert!(var.errors().is_none());
    var
}

fn flow(ident: &str, eqn: &str) -> Variable {
    use xmile::{Flow, Var};
    let x_flow = Var::Flow(Flow {
        name: ident.to_string(),
        eqn: Some(eqn.to_string()),
        doc: None,
        units: None,
        gf: None,
        non_negative: None,
        dimensions: None,
    });

    let var = parse_var(&x_flow);
    assert!(var.errors().is_none());
    var
}

fn aux(ident: &str, eqn: &str) -> Variable {
    use xmile::{Aux, Var};
    let x_aux = Var::Aux(Aux {
        name: ident.to_string(),
        eqn: Some(eqn.to_string()),
        doc: None,
        units: None,
        gf: None,
        dimensions: None,
    });

    let var = parse_var(&x_aux);
    assert!(var.errors().is_none());
    var
}

fn stock(ident: &str, eqn: &str, inflows: &[&str], outflows: &[&str]) -> Variable {
    use xmile::{Stock, Var};
    let x_stock = Var::Stock(Stock {
        name: ident.to_string(),
        eqn: Some(eqn.to_string()),
        doc: None,
        units: None,
        inflows: optional_vec(inflows),
        outflows: optional_vec(outflows),
        non_negative: None,
        dimensions: None,
    });

    let var = parse_var(&x_stock);
    assert!(var.errors().is_none());
    var
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
                    v.ident().clone(),
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

    let mod_1 = module("mod_1", &[("aux_3", "input")]);
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
