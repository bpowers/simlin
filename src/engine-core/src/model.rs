// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::ast;
use crate::xmile;

pub struct Table {
    x: Vec<f64>,
    y: Vec<f64>,
    x_range: Option<(f64, f64)>,
    y_range: Option<(f64, f64)>,
}

pub enum Variable {
    Stock {
        name: String,
        ast: Option<ast::Expr>,
        eqn: Option<String>,
        doc: Option<String>,
        units: Option<String>,
        inflows: Vec<String>,
        outflows: Vec<String>,
        non_negative: Option<bool>,
    },
    Flow {
        name: String,
        ast: Option<ast::Expr>,
        eqn: Option<String>,
        doc: Option<String>,
        units: Option<String>,
        table: Option<Table>,
        non_negative: Option<bool>,
    },
    Aux {
        name: String,
        ast: Option<ast::Expr>,
        eqn: Option<String>,
        doc: Option<String>,
        units: Option<String>,
        table: Option<Table>,
        non_negative: Option<bool>,
    },
    Module {
        name: String,
        doc: Option<String>,
        units: Option<String>,
        refs: Vec<xmile::Ref>,
    },
}

pub struct Model {
    pub name: String,
    pub variables: HashMap<String, Variable>,
    pub views: Vec<xmile::Views>,
}
