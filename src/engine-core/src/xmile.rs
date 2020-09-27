// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use serde::{Deserialize, Serialize};

// const VERSION: &str = "1.0";
// const NS_HTTPS: &str = "https://docs.oasis-open.org/xmile/ns/XMILE/v1.0";
// const NS_HTTP: &str = "http://docs.oasis-open.org/xmile/ns/XMILE/v1.0";

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename = "xmile")]
pub struct File {
    #[serde(default)]
    pub version: String,
    #[serde(rename = "xmlns", default)]
    pub namespace: String, // 'https://docs.oasis-open.org/xmile/ns/XMILE/v1.0'
    pub header: Option<Header>,
    pub sim_specs: Option<SimSpecs>,
    #[serde(rename = "model_units")]
    pub units: Option<Units>,
    pub dimensions: Option<Dimensions>,
    pub behavior: Option<Behavior>,
    pub style: Option<Style>,
    pub data: Option<Data>,
    #[serde(rename = "model", default)]
    pub models: Vec<Model>,
    #[serde(rename = "macro", default)]
    pub macros: Vec<Macro>,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Data {
    // TODO
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Macro {
    // TODO
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct VarDimensions {
    #[serde(rename = "dim")]
    pub dimensions: Option<Vec<VarDimension>>,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct VarDimension {
    pub name: String,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Dimensions {
    #[serde(rename = "dimension")]
    pub dimensions: Option<Vec<Dimension>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Header {
    pub vendor: String,
    pub product: Product,
    pub options: Option<Options>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub caption: Option<Caption>,
    pub image: Option<Image>,
    pub author: Option<String>,
    pub affiliation: Option<String>,
    pub client: Option<String>,
    pub copyright: Option<String>,
    pub created: Option<String>, // ISO 8601 date format, e.g. “ 2014-08-10”
    pub modified: Option<String>, // ISO 8601 date format
    pub uuid: Option<String>,    // IETF RFC4122 format (84-4-4-12 hex digits with the dashes)
    pub includes: Option<Includes>,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Caption {}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Includes {}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Image {
    #[serde(default)]
    pub resource: String, // "JPG, GIF, TIF, or PNG" path, URL, or image embedded in base64 data URI
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Product {
    #[serde(rename = "$value")]
    pub name: Option<String>,
    #[serde(rename = "lang")]
    pub language: Option<String>,
    pub version: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    UsesArrays {
        maximum_dimensions: Option<i64>,
        invalid_index_value: Option<String>, // e.g. "NaN" or "0"; string for Eq + Hash},
    },
    UsesMacros {
        recursive_macros: Option<bool>,
        option_filters: Option<bool>,
    },
    UsesConveyor {
        arrest: Option<bool>,
        leak: Option<bool>,
    },
    UsesQueue {
        overflow: Option<bool>,
    },
    UsesEventPosters {
        messages: Option<bool>,
    },
    HasModelView,
    UsesOutputs {
        numeric_display: Option<bool>,
        lamp: Option<bool>,
        gauge: Option<bool>,
    },
    UsesInputs {
        numeric_input: Option<bool>,
        list: Option<bool>,
        graphical_input: Option<bool>,
    },
    UsesAnnotation,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Options {
    pub namespace: Option<String>, // string of comma separated namespaces
    #[serde(rename = "$value")]
    pub features: Option<Vec<Feature>>,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct SimSpecs {
    pub start: f64,
    pub stop: f64,
    pub dt: Option<Dt>,
    #[serde(rename = "savestep")]
    pub save_step: Option<f64>,
    pub method: Option<String>,
    pub time_units: Option<String>,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct Dt {
    #[serde(rename = "$value")]
    pub value: f64,
    pub reciprocal: Option<bool>,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Dimension {
    pub name: String,
    pub size: Option<u32>,
    #[serde(rename = "elem")]
    pub elements: Option<Vec<Index>>,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Index {
    pub name: String,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TableType {
    Continuous,
    Extrapolate,
    Discrete,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct Scale {
    pub min: f64,
    pub max: f64,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct GF {
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<TableType>,
    #[serde(rename = "xscale")]
    pub x_scale: Option<Scale>,
    #[serde(rename = "yscale")]
    pub y_scale: Option<Scale>,
    #[serde(rename = "xpts")]
    pub x_pts: Option<String>, // comma separated list of points
    #[serde(rename = "ypts")]
    pub y_pts: Option<String>, // comma separated list of points
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Behavior {
    // TODO
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Style {
    // TODO
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Units {
    pub unit: Option<Vec<Unit>>,
}
#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Unit {
    pub name: String,
    pub eqn: Option<String>,
    pub alias: Option<Vec<String>>,
    pub disabled: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Model {
    pub name: Option<String>,
    #[serde(rename = "namespace")]
    pub namespaces: Option<String>, // comma separated list of namespaces
    pub resource: Option<String>, // path or URL to separate resource file
    pub sim_specs: Option<SimSpecs>,
    pub variables: Option<Variables>,
    pub views: Option<Views>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct Variables {
    #[serde(rename = "$value")]
    pub variables: Vec<Var>,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct Views {
    pub view: Option<Vec<View>>,
}

impl Model {
    #[allow(dead_code)] // TODO: false positive
    pub fn get_name(&self) -> &str {
        &self.name.as_deref().unwrap_or("main")
    }
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewType {
    StockFlow,
    Interface,
    Popup,
    VendorSpecific,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelSide {
    Top,
    Left,
    Center,
    Bottom,
    Right,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct Point {
    x: f64,
    y: f64,
    uid: Option<i32>,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct Points {
    #[serde(rename = "pt")]
    points: Vec<Point>,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewObject {
    Stock {
        name: String,
        uid: Option<i32>,
        x: f64,
        y: f64,
        width: Option<f64>,
        height: Option<f64>,
        label_side: Option<LabelSide>,
        label_angle: Option<f64>,
    },
    Flow {
        name: String,
        uid: Option<i32>,
        x: f64,
        y: f64,
        width: Option<f64>,
        height: Option<f64>,
        label_side: Option<LabelSide>,
        label_angle: Option<f64>,
        #[serde(rename = "pts")]
        points: Option<Points>,
    },
    Aux {
        name: String,
        uid: Option<i32>,
        x: f64,
        y: f64,
        width: Option<f64>,
        height: Option<f64>,
        label_side: Option<LabelSide>,
        label_angle: Option<f64>,
    },
    Connector {
        uid: Option<i32>,
        label_side: Option<LabelSide>,
        label_angle: Option<f64>,
        from: String,
        to: String,
        angle: Option<f64>,
        #[serde(rename = "pts")]
        points: Option<Points>, // for multi-point connectors
    },
    Module {
        name: String,
        uid: Option<i32>,
        x: f64,
        y: f64,
    },
    Style(Style),
    StackedContainer,
    SimulationDelay,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct View {
    #[serde(rename = "type")]
    pub kind: Option<ViewType>,
    pub background: Option<String>,
    pub page_width: Option<String>,
    pub page_height: Option<String>,
    pub show_pages: Option<bool>,
    #[serde(rename = "$value", default)]
    pub objects: Vec<ViewObject>,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct ArrayElement {
    pub subscript: String,
    pub eqn: String,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Module {
    pub name: String,
    pub doc: Option<String>,
    pub units: Option<String>,
    #[serde(rename = "$value", default)]
    pub refs: Vec<Reference>,
}
#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reference {
    // these only differ in the semantics of their contents
    Connect(Connect),
    Connect2(Connect),
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Connect {
    #[serde(rename = "from")]
    pub src: String,
    #[serde(rename = "to")]
    pub dst: String,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct NonNegative {}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Stock {
    pub name: String,
    pub eqn: Option<String>,
    pub doc: Option<String>,
    pub units: Option<String>,
    #[serde(rename = "inflow")]
    pub inflows: Option<Vec<String>>,
    #[serde(rename = "outflow")]
    pub outflows: Option<Vec<String>>,
    pub non_negative: Option<NonNegative>,
    pub dimensions: Option<VarDimensions>,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct Flow {
    pub name: String,
    pub eqn: Option<String>,
    pub doc: Option<String>,
    pub units: Option<String>,
    pub gf: Option<GF>,
    pub non_negative: Option<NonNegative>,
    pub dimensions: Option<VarDimensions>,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct Aux {
    pub name: String,
    pub eqn: Option<String>,
    pub doc: Option<String>,
    pub units: Option<String>,
    pub gf: Option<GF>,
    pub dimensions: Option<VarDimensions>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Var {
    Stock(Stock),
    Flow(Flow),
    Aux(Aux),
    Module(Module),
}

impl Var {
    #[allow(dead_code)] // this is a false-positive lint
    pub fn get_noncanonical_name(&self) -> &str {
        match self {
            Var::Stock(stock) => stock.name.as_str(),
            Var::Flow(flow) => flow.name.as_str(),
            Var::Aux(aux) => aux.name.as_str(),
            Var::Module(module) => module.name.as_str(),
        }
    }
}
