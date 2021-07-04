// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::result::Result as StdResult;

use crate::common::Error;
use crate::units::Context;
use crate::Project;

#[allow(dead_code)]
pub fn check(_project: &Project, _ctx: Context, _model_name: &str) -> StdResult<(), Vec<Error>> {
    // units checking uses the model's equations and variable's
    // unit definitions to calculate the concrete units for each
    // equation.  If these don't match the units as defined, we
    // log an error.
    Err(vec![])
}
