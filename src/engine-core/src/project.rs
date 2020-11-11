// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// #![allow(dead_code)]

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum GraphicalFunctionKind {
    Continuous,
    Extrapolate,
    Discrete,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GraphicalFunctionScale {
    pub min: f64,
    pub max: f64,
}

pub struct GraphicalFunction {
    pub kind: GraphicalFunctionKind,
    pub y_points: Vec<f64>,
    pub x_scale: GraphicalFunctionScale,
    pub y_scale: GraphicalFunctionScale,
}

pub struct Stock {
    pub ident: String,
    pub equation: String,
    pub documentation: String,
    pub units: Option<String>,
    pub inflows: Vec<String>,
    pub outflows: Vec<String>,
    pub non_negative: bool,
}

pub struct Flow {
    pub ident: String,
    pub equation: String,
    pub documentation: String,
    pub units: Option<String>,
    pub gf: Option<GraphicalFunction>,
    pub non_negative: bool,
}

pub struct Aux {
    pub ident: String,
    pub equation: String,
    pub documentation: String,
    pub gf: Option<GraphicalFunction>,
    pub units: Option<String>,
}

pub struct ModuleReference {
    pub src: String,
    pub dst: String,
}

pub struct Module {
    pub ident: String,
    pub model_name: String,
    pub documentation: String,
    pub units: Option<String>,
    pub references: Vec<ModuleReference>,
}

pub enum Variable {
    Stock(Stock),
    Flow(Flow),
    Aux(Aux),
    Module(Module),
}

pub mod view_element {
    pub enum LabelSide {
        Top,
        Left,
        Center,
        Bottom,
        Right,
    }

    pub struct Aux {
        pub name: String,
        pub uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
    }

    pub struct Stock {
        pub name: String,
        pub uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
    }

    pub struct FlowPoint {
        pub x: f64,
        pub y: f64,
        pub attached_to_uid: Option<i32>,
    }

    pub struct Flow {
        pub name: String,
        pub uid: i32,
        pub segment_with_aux: i32,
        pub aux_percentage_into_segment: f64,
        pub points: Vec<FlowPoint>,
    }

    pub enum LinkShape {
        Straight,
        Curved(f64), // angle in [0, 360)
    }

    pub struct Link {
        pub from_uid: i32,
        pub to_uid: i32,
        pub shape: LinkShape,
    }

    pub struct Module {
        pub name: String,
        pub uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
    }

    pub struct Alias {
        pub uid: i32,
        pub alias_of_uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
    }

    pub struct Cloud {
        pub uid: i32,
        pub flow_uid: i32,
        pub x: f64,
        pub y: f64,
    }
}

pub enum ViewElement {
    Aux(view_element::Aux),
    Stock(view_element::Stock),
    Flow(view_element::Flow),
    Link(view_element::Link),
    Module(view_element::Module),
    Alias(view_element::Alias),
    Cloud(view_element::Cloud),
}

pub struct StockFlow {
    pub elements: Vec<ViewElement>,
}

pub enum View {
    StockFlow(StockFlow),
}

pub struct Model {
    pub name: String,
    pub variables: Vec<Variable>,
    pub views: Vec<View>,
}

pub enum SimMethod {
    Euler,
}

pub struct SimSpecs {
    pub start: f64,
    pub stop: f64,
    pub dt: f64,
    pub save_step: Option<f64>,
    pub sim_method: SimMethod,
    pub time_units: Option<String>,
}

pub struct Project {
    pub name: String,
    pub sim_specs: SimSpecs,
    pub models: Vec<Model>,
}
