// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum GraphicalFunctionKind {
    Continuous,
    Extrapolate,
    Discrete,
}

#[derive(Clone, PartialEq, Debug)]
pub struct GraphicalFunctionScale {
    pub min: f64,
    pub max: f64,
}

#[derive(Clone, PartialEq, Debug)]
pub struct GraphicalFunction {
    pub kind: GraphicalFunctionKind,
    pub y_points: Vec<f64>,
    pub x_scale: GraphicalFunctionScale,
    pub y_scale: GraphicalFunctionScale,
}

#[allow(dead_code)]
pub struct Stock {
    pub ident: String,
    pub equation: String,
    pub documentation: String,
    pub units: Option<String>,
    pub inflows: Vec<String>,
    pub outflows: Vec<String>,
    pub non_negative: bool,
}

#[allow(dead_code)]
pub struct Flow {
    pub ident: String,
    pub equation: String,
    pub documentation: String,
    pub units: Option<String>,
    pub gf: Option<GraphicalFunction>,
    pub non_negative: bool,
}

#[allow(dead_code)]
pub struct Aux {
    pub ident: String,
    pub equation: String,
    pub documentation: String,
    pub gf: Option<GraphicalFunction>,
    pub units: Option<String>,
}

#[allow(dead_code)]
pub struct ModuleReference {
    pub src: String,
    pub dst: String,
}

#[allow(dead_code)]
pub struct Module {
    pub ident: String,
    pub model_name: String,
    pub documentation: String,
    pub units: Option<String>,
    pub references: Vec<ModuleReference>,
}

#[allow(dead_code)]
pub enum Variable {
    Stock(Stock),
    Flow(Flow),
    Aux(Aux),
    Module(Module),
}

pub mod view_element {
    #[allow(dead_code)]
    pub enum LabelSide {
        Top,
        Left,
        Center,
        Bottom,
        Right,
    }

    #[allow(dead_code)]
    pub struct Aux {
        pub name: String,
        pub uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
    }

    #[allow(dead_code)]
    pub struct Stock {
        pub name: String,
        pub uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
    }

    #[allow(dead_code)]
    pub struct FlowPoint {
        pub x: f64,
        pub y: f64,
        pub attached_to_uid: Option<i32>,
    }

    #[allow(dead_code)]
    pub struct Flow {
        pub name: String,
        pub uid: i32,
        pub segment_with_aux: i32,
        pub aux_percentage_into_segment: f64,
        pub points: Vec<FlowPoint>,
    }

    #[allow(dead_code)]
    pub enum LinkShape {
        Straight,
        Curved(f64), // angle in [0, 360)
    }

    #[allow(dead_code)]
    pub struct Link {
        pub from_uid: i32,
        pub to_uid: i32,
        pub shape: LinkShape,
    }

    #[allow(dead_code)]
    pub struct Module {
        pub name: String,
        pub uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
    }

    #[allow(dead_code)]
    pub struct Alias {
        pub uid: i32,
        pub alias_of_uid: i32,
        pub x: f64,
        pub y: f64,
        pub label_side: LabelSide,
    }

    #[allow(dead_code)]
    pub struct Cloud {
        pub uid: i32,
        pub flow_uid: i32,
        pub x: f64,
        pub y: f64,
    }
}

#[allow(dead_code)]
pub enum ViewElement {
    Aux(view_element::Aux),
    Stock(view_element::Stock),
    Flow(view_element::Flow),
    Link(view_element::Link),
    Module(view_element::Module),
    Alias(view_element::Alias),
    Cloud(view_element::Cloud),
}

#[allow(dead_code)]
pub struct StockFlow {
    pub elements: Vec<ViewElement>,
}

#[allow(dead_code)]
pub enum View {
    StockFlow(StockFlow),
}

#[allow(dead_code)]
pub struct Model {
    pub name: String,
    pub variables: Vec<Variable>,
    pub views: Vec<View>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SimMethod {
    Euler,
    RungeKutta4,
}

#[derive(Clone, PartialEq, Debug)]
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

#[derive(Clone, PartialEq, Debug)]
pub struct SimSpecs {
    pub start: f64,
    pub stop: f64,
    pub dt: Dt,
    pub save_step: Option<f64>,
    pub sim_method: SimMethod,
    pub time_units: Option<String>,
}

#[allow(dead_code)]
pub struct Project {
    pub name: String,
    pub sim_specs: SimSpecs,
    pub models: Vec<Model>,
}
