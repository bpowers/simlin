// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::Expr;
use crate::common::Ident;
use crate::xmile;
use std::collections::HashMap;

// FIXME: remove when done here
#[allow(dead_code)]
pub struct BuiltinVisitor<'a> {
    variable_name: &'a str,
    models: &'a HashMap<String, HashMap<Ident, &'a xmile::Var>>,
    pub modules: HashMap<Ident, xmile::Var>,
    pub vars: HashMap<Ident, xmile::Var>,
    n: usize,
}

impl<'a> BuiltinVisitor<'a> {
    // FIXME: remove when done here
    #[allow(dead_code)]
    pub fn new(
        variable_name: &'a str,
        _ast: &Expr,
        models: &'a HashMap<String, HashMap<Ident, &'a xmile::Var>>,
    ) -> Self {
        Self {
            variable_name,
            models,
            modules: Default::default(),
            vars: Default::default(),
            n: 0,
        }
    }
}

#[test]
fn test_builtin_visitor() {
    let _visitor = BuiltinVisitor::new(
        "test",
        &Expr::Const("0.0".to_string(), 0.0),
        &HashMap::new(),
    );
}
