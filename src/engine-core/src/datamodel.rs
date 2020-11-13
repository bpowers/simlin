// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub enum GraphicalFunctionKind {
    Continuous,
    Extrapolate,
    Discrete,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct GraphicalFunctionScale {
    pub min: f64,
    pub max: f64,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct GraphicalFunction {
    pub kind: GraphicalFunctionKind,
    pub y_points: Vec<f64>,
    pub x_points: Option<Vec<f64>>,
    pub x_scale: GraphicalFunctionScale,
    pub y_scale: GraphicalFunctionScale,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Stock {
    pub ident: String,
    pub equation: String,
    pub documentation: String,
    pub units: Option<String>,
    pub inflows: Vec<String>,
    pub outflows: Vec<String>,
    pub non_negative: bool,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct Flow {
    pub ident: String,
    pub equation: String,
    pub documentation: String,
    pub units: Option<String>,
    pub gf: Option<GraphicalFunction>,
    pub non_negative: bool,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct Aux {
    pub ident: String,
    pub equation: String,
    pub documentation: String,
    pub gf: Option<GraphicalFunction>,
    pub units: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct ModuleReference {
    pub src: String,
    pub dst: String,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Module {
    pub ident: String,
    pub model_name: String,
    pub documentation: String,
    pub units: Option<String>,
    pub references: Vec<ModuleReference>,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub enum Variable {
    Stock(Stock),
    Flow(Flow),
    Aux(Aux),
    Module(Module),
}

impl Variable {
    #[allow(dead_code)] // this is a false-positive lint
    pub fn get_ident(&self) -> &str {
        match self {
            Variable::Stock(stock) => stock.ident.as_str(),
            Variable::Flow(flow) => flow.ident.as_str(),
            Variable::Aux(aux) => aux.ident.as_str(),
            Variable::Module(module) => module.ident.as_str(),
        }
    }
}

pub mod view_element {
    use serde::{Deserialize, Serialize};

    #[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
    pub enum LabelSide {
        Top,
        Left,
        Center,
        Bottom,
        Right,
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Aux {
        pub name: String,
        pub uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Stock {
        pub name: String,
        pub uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct FlowPoint {
        pub x: f64,
        pub y: f64,
        pub attached_to_uid: Option<i32>,
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Flow {
        pub name: String,
        pub uid: i32,
        pub segment_with_aux: i32,
        pub aux_percentage_into_segment: f64,
        pub points: Vec<FlowPoint>,
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub enum LinkShape {
        Straight,
        Curved(f64), // angle in [0, 360)
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Link {
        pub from_uid: i32,
        pub to_uid: i32,
        pub shape: LinkShape,
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Module {
        pub name: String,
        pub uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Alias {
        pub uid: i32,
        pub alias_of_uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Cloud {
        pub uid: i32,
        pub flow_uid: i32,
        pub x: f64,
        pub y: f64,
    }
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub enum ViewElement {
    Aux(view_element::Aux),
    Stock(view_element::Stock),
    Flow(view_element::Flow),
    Link(view_element::Link),
    Module(view_element::Module),
    Alias(view_element::Alias),
    Cloud(view_element::Cloud),
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct StockFlow {
    pub elements: Vec<ViewElement>,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub enum View {
    StockFlow(StockFlow),
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct Model {
    pub name: String,
    pub variables: Vec<Variable>,
    pub views: Vec<View>,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub enum SimMethod {
    Euler,
    RungeKutta4,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub enum Dt {
    Dt(f64),
    Reciprocal(f64),
}

/// The default dt is 1, just like XMILE
impl Default for Dt {
    fn default() -> Self {
        Dt::Dt(1.0)
    }
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct SimSpecs {
    pub start: f64,
    pub stop: f64,
    pub dt: Dt,
    pub save_step: Option<f64>,
    pub sim_method: SimMethod,
    pub time_units: Option<String>,
}

#[allow(dead_code)]
#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct Project {
    pub name: String,
    pub sim_specs: SimSpecs,
    pub models: Vec<Model>,
}
