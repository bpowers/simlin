// Copyright 2021 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeMap, HashMap};

use crate::ast::parse_equation;
use crate::common::Result;
use crate::datamodel::Unit;

type UnitMap = BTreeMap<String, i32>;

#[allow(dead_code)]
pub struct Context {
    aliases: HashMap<String, String>,
    units: HashMap<String, Option<UnitMap>>,
}

impl Context {
    #[allow(dead_code, clippy::unnecessary_wraps)]
    fn new(units: &[Unit]) -> Result<Self> {
        // step 1: build our base context consisting of all prime units
        let mut aliases = HashMap::new();
        let mut parsed_units = HashMap::new();
        for unit in units.iter().filter(|unit| unit.equation.is_none()) {
            for alias in unit.aliases.iter() {
                aliases.insert(alias.clone(), unit.name.clone());
            }
            parsed_units.insert(unit.name.clone(), None);
        }

        let mut ctx = Context {
            aliases,
            units: parsed_units,
        };

        // step 2: use this base context to parse our units with equations
        for unit in units.iter().filter(|unit| unit.equation.is_some()) {
            for alias in unit.aliases.iter() {
                ctx.aliases.insert(alias.clone(), unit.name.clone());
            }

            let eqn = unit.equation.as_ref().unwrap();

            let (_ast, _errors) = parse_equation(eqn);

            // then using the Context turn the equation into a UnitMap

            let unit_components = UnitMap::new();

            ctx.units.insert(unit.name.clone(), Some(unit_components));
        }

        Ok(ctx)
    }
}

#[test]
fn test_basic_unit_checks() {
    // from a set of datamodel::Units build a Context

    // with a context, check if a set of variables unit checks
}
