// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#[cfg(test)]
use std::collections::HashMap;

use crate::common::Result;
#[cfg(test)]
use crate::model::ModelStage0;
use crate::model::ModelStage1;
#[cfg(test)]
use crate::testutils::{x_aux, x_flow, x_model, x_stock};
#[cfg(test)]
use crate::units::Context;

#[allow(dead_code)]
fn infer(_model: &ModelStage1) -> Result<()> {
    Ok(())
}

#[test]
fn test_inference() {
    let sim_specs = crate::datamodel::SimSpecs {
        start: 0.0,
        stop: 0.0,
        dt: Default::default(),
        save_step: None,
        sim_method: Default::default(),
        // if star wars says its a time unit, that's good enough for me
        time_units: Some("parsec".to_owned()),
    };

    // we should be able to fully infer the units here:
    // - window needs to be "parsec"
    // - seen needs to be "usd"
    // - and inflow needs to be "usd/parsec"
    let model = x_model(
        "main",
        vec![
            x_stock("stock_1", "1", &["inflow"], &[], Some("usd")),
            x_aux("window", "6", None),
            x_flow("inflow", "seen/window", None),
            x_aux("seen", "3", None),
        ],
    );

    let units_ctx = Context::new_with_builtins(&[], &sim_specs).unwrap();

    let model = ModelStage0::new(&model, &[], &units_ctx, false);
    let model = ModelStage1::new(&units_ctx, &HashMap::new(), &model);

    infer(&model).unwrap();
}
