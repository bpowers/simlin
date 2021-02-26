// Copyright 2021 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::BTreeMap;

use crate::ast::Expr;

#[allow(dead_code)]
pub struct Context {
    units: BTreeMap<String, Option<Expr>>,
}

#[test]
fn test_basic_unit_checks() {
    // from a set of datamodel::Units build a Context

    // with a context, check if a set of variables unit checks
}
