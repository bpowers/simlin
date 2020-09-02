// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{HashMap, HashSet};

use crate::common::Ident;
use crate::variable::{parse_var, Variable};
use crate::xmile;

#[derive(Debug, PartialEq)]
pub struct ModelError {
    pub ident: Option<Ident>,
    pub msg: String,
}

#[derive(Debug)]
pub struct Model {
    pub name: String,
    pub variables: HashMap<String, Variable>,
    pub errors: Option<Vec<ModelError>>,
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
fn all_deps<'a>(
    vars: &'a [Variable],
    is_initial: bool,
) -> Result<HashMap<Ident, HashSet<Ident>>, ModelError> {
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
    ) -> Result<(), ModelError> {
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
        let mut all_deps = var.direct_deps().clone();

        for dep in var.direct_deps().iter().map(|d| d.as_str()) {
            if !all_vars.contains_key(dep) {
                // TODO: this is probably an error
                continue;
            }

            // ensure we don't blow the stack
            if processing.contains(dep) {
                return Err(ModelError {
                    ident: Some(id.to_string()),
                    msg: format!("recursive dependency with {}", dep),
                });
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

        let mut errors: Vec<ModelError> = Vec::new();

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
