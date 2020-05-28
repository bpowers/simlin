// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::ast;
use crate::eqn::ProgramParser;
use crate::xmile;

#[derive(Debug)]
pub struct Table {
    x: Vec<f64>,
    y: Vec<f64>,
    x_range: Option<(f64, f64)>,
    y_range: Option<(f64, f64)>,
}

#[derive(Debug)]
pub enum Variable {
    Stock {
        name: String,
        ast: Option<ast::Expr>,
        eqn: Option<String>,
        units: Option<String>,
        inflows: Vec<String>,
        outflows: Vec<String>,
        non_negative: bool,
    },
    Var {
        name: String,
        ast: Option<ast::Expr>,
        eqn: Option<String>,
        units: Option<String>,
        table: Option<Table>,
        non_negative: bool,
        is_flow: bool,
        is_table_only: bool,
    },
    Module {
        name: String,
        units: Option<String>,
        refs: Vec<xmile::Ref>,
    },
}

impl Variable {
    pub fn name(&self) -> &String {
        match self {
            Variable::Stock { name, .. } => name,
            Variable::Var { name, .. } => name,
            Variable::Module { name, .. } => name,
        }
    }

    pub fn eqn(&self) -> Option<&String> {
        match self {
            Variable::Stock { eqn: Some(s), .. } => Some(s),
            Variable::Var { eqn: Some(s), .. } => Some(s),
            _ => None,
        }
    }

    fn parse_eqn(&mut self) {
        if let Variable::Module { .. } = self {
            return;
        }

        let eqn = self.eqn();
        if eqn.is_none() {
            return;
        }

        let eqn = eqn.unwrap();
        let lexer = crate::token::Tokenizer::new(eqn);
        match ProgramParser::new().parse(eqn, lexer) {
            Ok(ast) => (),
            Err(err) => (),
        }
    }
}

#[derive(Debug)]
pub struct Model {
    pub name: String,
    pub variables: HashMap<String, Variable>,
    pub views: Vec<xmile::Views>,
}

const EMPTY_VARS: xmile::Variables = xmile::Variables {
    variables: Vec::new(),
};

impl Model {
    pub fn new(x_model: &xmile::Model) -> Self {
        let mut variable_list: Vec<Variable> = x_model
            .variables
            .as_ref()
            .unwrap_or(&EMPTY_VARS)
            .variables
            .iter()
            .map(|v| match v {
                xmile::Var::Stock(v) => Variable::Stock {
                    name: v.name.clone(),
                    ast: None,
                    eqn: v.eqn.clone(),
                    units: v.units.clone(),
                    inflows: v.inflows.clone().unwrap_or(Vec::new()),
                    outflows: v.outflows.clone().unwrap_or(Vec::new()),
                    non_negative: v.non_negative.is_some(),
                },
                xmile::Var::Flow(v) => Variable::Var {
                    name: v.name.clone(),
                    ast: None,
                    eqn: v.eqn.clone(),
                    units: v.units.clone(),
                    table: None,
                    is_flow: true,
                    is_table_only: false,
                    non_negative: v.non_negative.is_some(),
                },
                xmile::Var::Aux(v) => Variable::Var {
                    name: v.name.clone(),
                    ast: None,
                    eqn: v.eqn.clone(),
                    units: v.units.clone(),
                    table: None,
                    is_flow: false,
                    is_table_only: false,
                    non_negative: false,
                },
                xmile::Var::Module(v) => Variable::Module {
                    name: v.name.clone(),
                    units: v.units.clone(),
                    refs: v.refs.clone().unwrap_or(Vec::new()),
                },
            })
            .collect();

        for v in variable_list.iter_mut() {
            v.parse_eqn()
        }

        let m = Model {
            name: x_model.name.as_ref().unwrap_or(&"main".to_string()).clone(),
            variables: variable_list
                .into_iter()
                .map(|v| (v.name().clone(), v))
                .collect(),
            views: Vec::new(),
        };

        m
    }
}
