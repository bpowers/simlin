// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::variable::{parse_var, Variable};
use crate::xmile;

#[derive(Debug)]
pub struct Model {
    pub name: String,
    pub variables: HashMap<String, Variable>,
}

const EMPTY_VARS: xmile::Variables = xmile::Variables {
    variables: Vec::new(),
};

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

        Model {
            name: x_model.name.as_ref().unwrap_or(&"main".to_string()).clone(),
            variables: variable_list
                .into_iter()
                .map(|v| (v.ident().clone(), v))
                .collect(),
        }
    }
}
