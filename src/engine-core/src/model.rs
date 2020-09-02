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
}

const EMPTY_VARS: xmile::Variables = xmile::Variables {
    variables: Vec::new(),
};

// to ensure we sort the list of variables in O(n*log(n)) time, we
// need to iterate over the set of variables we have and compute
// their recursive dependencies.  (assuming this function runs
// in <= O(n*log(n)))
fn all_deps(
    vars: &Vec<Variable>,
    _is_initial: bool,
) -> Result<HashMap<Ident, HashSet<Ident>>, ModelError> {
    let mut processing: HashSet<Ident> = HashSet::new();
    let mut all_vars: HashMap<Ident, &Variable> =
        vars.iter().map(|v| (v.ident().clone(), v)).collect();
    let mut all_var_deps: HashMap<Ident, Option<HashSet<Ident>>> =
        vars.iter().map(|v| (v.ident().clone(), None)).collect();

    fn all_deps_inner(
        id: &Ident,
        processing: &mut HashSet<Ident>,
        all_vars: &mut HashMap<Ident, &Variable>,
        all_var_deps: &mut HashMap<Ident, Option<HashSet<Ident>>>,
    ) -> Result<(), ModelError> {
        let var = all_vars[id];

        // short circuit if we've already figured this out
        if let Some(_) = all_var_deps[id] {
            return Ok(());
        }

        // dependency chains break at stocks, as we use their value from the
        // last dt timestep.
        if let Variable::Stock { .. } = var {
            all_var_deps.insert(id.clone(), Some(HashSet::new()));
            return Ok(());
        }

        processing.insert(id.clone());

        // all deps start out as the direct deps
        let mut all_deps = var.direct_deps().clone();

        for dep in var.direct_deps() {
            if !all_vars.contains_key(dep) {
                // TODO: this is probably an error
                continue;
            }

            // ensure we don't blow the stack
            if processing.contains(dep) {
                return Err(ModelError {
                    ident: Some(id.clone()),
                    msg: format!("recursive dependency with {}", dep),
                });
            }

            if let None = all_var_deps[dep] {
                all_deps_inner(dep, processing, all_vars, all_var_deps)?;
            }

            let dep_deps = all_var_deps[dep].as_ref().unwrap();
            all_deps.extend(dep_deps.iter().map(|id| id.clone()));
        }

        processing.remove(id);

        all_var_deps.insert(id.clone(), Some(all_deps));

        Ok(())
    };

    for var in vars.iter() {
        all_deps_inner(
            var.ident(),
            &mut processing,
            &mut all_vars,
            &mut all_var_deps,
        )?;
    }

    // this unwrap is safe, because of the full iteration over vars directly above
    let var_deps: HashMap<Ident, HashSet<Ident>> = all_var_deps
        .into_iter()
        .map(|(k, v)| (k.clone(), v.unwrap()))
        .collect();

    Ok(var_deps)
}

impl Model {
    pub fn new(x_model: &xmile::Model) -> Self {
        let mut variable_list: Vec<Variable> = x_model
            .variables
            .as_ref()
            .unwrap_or(&EMPTY_VARS)
            .variables
            .iter()
            .map(parse_var)
            .collect();

        let mut errors: Vec<ModelError> = Vec::new();

        match all_deps(&mut variable_list, false) {
            Ok(_) => (),
            Err(err) => errors.push(err),
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
        }
    }
}
