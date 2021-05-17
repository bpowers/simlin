// Copyright 2021 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#[derive(PartialEq, Clone, Debug, Default)]
pub struct File {
    pub macros: Vec<Macro>,
    // there may be multiple variable definitions for
    // subscripted variables, but this struct is intended
    // to match the on-disk format
    pub variables: Vec<Variable>,
    pub control: Vec<Variable>,
}

#[derive(PartialEq, Clone, Debug, Default)]
pub struct Macro {
    pub name: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub variables: Vec<Variable>,
}

#[derive(PartialEq, Clone, Debug, Default)]
pub struct Variable {
    pub name: String,
    pub equation: String,
    pub units: String,
    pub comment: String,
    pub range: Option<Range>,
}

#[derive(PartialEq, Clone, Copy, Debug, Default)]
pub struct Range {
    pub min: f64,
    pub max: f64,
    pub increment: Option<f64>,
}

#[derive(PartialEq, Clone, Debug)]
pub enum Definition {
    Macro(Macro),
    Variable(Variable),
}
