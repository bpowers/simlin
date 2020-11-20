// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::io::BufRead;

use serde::{Deserialize, Serialize};

use engine_core::common::{canonicalize, Result};
use engine_core::datamodel;

macro_rules! import_err(
    ($code:tt, $str:expr) => {{
        use engine_core::common::{Error, ErrorCode};
        Err(Error::ImportError(ErrorCode::$code, $str))
    }}
);

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

impl From<File> for datamodel::Project {
    fn from(file: File) -> Self {
        datamodel::Project {
            name: "".to_string(),
            sim_specs: datamodel::SimSpecs::from(file.sim_specs.unwrap_or(SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: Some(Dt {
                    value: 1.0,
                    reciprocal: None,
                }),
                save_step: None,
                method: None,
                time_units: None,
            })),
            models: file
                .models
                .into_iter()
                .map(datamodel::Model::from)
                .collect(),
        }
    }
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

impl From<SimSpecs> for datamodel::SimSpecs {
    fn from(sim_specs: SimSpecs) -> Self {
        let sim_method = sim_specs
            .method
            .unwrap_or_else(|| "euler".to_string())
            .to_lowercase();
        datamodel::SimSpecs {
            start: sim_specs.start,
            stop: sim_specs.stop,
            dt: match sim_specs.dt {
                Some(dt) => datamodel::Dt::from(dt),
                None => Default::default(),
            },
            save_step: match sim_specs.save_step {
                Some(save_step) => Some(datamodel::Dt::Dt(save_step)),
                None => None,
            },
            // FIXME: the spec says method is technically a
            //   comma separated list of fallbacks
            sim_method: match sim_method.as_str() {
                "euler" => datamodel::SimMethod::Euler,
                "rk4" => datamodel::SimMethod::RungeKutta4,
                _ => datamodel::SimMethod::Euler,
            },
            time_units: sim_specs.time_units,
        }
    }
}

impl From<datamodel::SimSpecs> for SimSpecs {
    fn from(sim_specs: datamodel::SimSpecs) -> Self {
        SimSpecs {
            start: sim_specs.start,
            stop: sim_specs.stop,
            dt: Some(Dt::from(sim_specs.dt)),
            save_step: match sim_specs.save_step {
                None => None,
                Some(dt) => match dt {
                    datamodel::Dt::Dt(value) => Some(value),
                    datamodel::Dt::Reciprocal(value) => Some(1.0 / value),
                },
            },
            method: Some(match sim_specs.sim_method {
                datamodel::SimMethod::Euler => "euler".to_string(),
                datamodel::SimMethod::RungeKutta4 => "rk4".to_string(),
            }),
            time_units: sim_specs.time_units,
        }
    }
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct Dt {
    #[serde(rename = "$value")]
    pub value: f64,
    pub reciprocal: Option<bool>,
}

impl From<Dt> for datamodel::Dt {
    fn from(dt: Dt) -> Self {
        if dt.reciprocal.unwrap_or(false) {
            datamodel::Dt::Reciprocal(dt.value)
        } else {
            datamodel::Dt::Dt(dt.value)
        }
    }
}

impl From<datamodel::Dt> for Dt {
    fn from(dt: datamodel::Dt) -> Self {
        match dt {
            datamodel::Dt::Dt(value) => Dt {
                value,
                reciprocal: None,
            },
            datamodel::Dt::Reciprocal(value) => Dt {
                value,
                reciprocal: Some(true),
            },
        }
    }
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

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct GraphicalFunctionScale {
    pub min: f64,
    pub max: f64,
}

impl From<GraphicalFunctionScale> for datamodel::GraphicalFunctionScale {
    fn from(scale: GraphicalFunctionScale) -> Self {
        datamodel::GraphicalFunctionScale {
            min: scale.min,
            max: scale.max,
        }
    }
}

impl From<datamodel::GraphicalFunctionScale> for GraphicalFunctionScale {
    fn from(scale: datamodel::GraphicalFunctionScale) -> Self {
        GraphicalFunctionScale {
            min: scale.min,
            max: scale.max,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphicalFunctionKind {
    Continuous,
    Extrapolate,
    Discrete,
}

impl From<GraphicalFunctionKind> for datamodel::GraphicalFunctionKind {
    fn from(kind: GraphicalFunctionKind) -> Self {
        match kind {
            GraphicalFunctionKind::Continuous => datamodel::GraphicalFunctionKind::Continuous,
            GraphicalFunctionKind::Extrapolate => datamodel::GraphicalFunctionKind::Extrapolate,
            GraphicalFunctionKind::Discrete => datamodel::GraphicalFunctionKind::Discrete,
        }
    }
}

impl From<datamodel::GraphicalFunctionKind> for GraphicalFunctionKind {
    fn from(kind: datamodel::GraphicalFunctionKind) -> Self {
        match kind {
            datamodel::GraphicalFunctionKind::Continuous => GraphicalFunctionKind::Continuous,
            datamodel::GraphicalFunctionKind::Extrapolate => GraphicalFunctionKind::Extrapolate,
            datamodel::GraphicalFunctionKind::Discrete => GraphicalFunctionKind::Discrete,
        }
    }
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct GF {
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<GraphicalFunctionKind>,
    #[serde(rename = "xscale")]
    pub x_scale: Option<GraphicalFunctionScale>,
    #[serde(rename = "yscale")]
    pub y_scale: Option<GraphicalFunctionScale>,
    #[serde(rename = "xpts")]
    pub x_pts: Option<String>, // comma separated list of points
    #[serde(rename = "ypts")]
    pub y_pts: Option<String>, // comma separated list of points
}

impl From<GF> for datamodel::GraphicalFunction {
    fn from(gf: GF) -> Self {
        use std::str::FromStr;

        let kind = datamodel::GraphicalFunctionKind::from(
            gf.kind.unwrap_or(GraphicalFunctionKind::Continuous),
        );

        let x_points: std::result::Result<Vec<f64>, _> = match &gf.x_pts {
            None => Ok(vec![]),
            Some(x_pts) => x_pts.split(',').map(|n| f64::from_str(n.trim())).collect(),
        };
        let x_points: Vec<f64> = match x_points {
            Ok(pts) => pts,
            Err(_) => vec![],
        };

        let y_points: std::result::Result<Vec<f64>, _> = match &gf.y_pts {
            None => Ok(vec![]),
            Some(y_pts) => y_pts.split(',').map(|n| f64::from_str(n.trim())).collect(),
        };
        let y_points: Vec<f64> = match y_points {
            Ok(pts) => pts,
            Err(_) => vec![],
        };

        let x_scale = match gf.x_scale {
            Some(x_scale) => datamodel::GraphicalFunctionScale::from(x_scale),
            None => {
                let min = if x_points.is_empty() {
                    0.0
                } else {
                    x_points.iter().fold(f64::INFINITY, |a, &b| a.min(b))
                };
                let max = if x_points.is_empty() {
                    1.0
                } else {
                    x_points.iter().fold(-f64::INFINITY, |a, &b| a.max(b))
                };
                datamodel::GraphicalFunctionScale { min, max }
            }
        };

        let y_scale = match gf.y_scale {
            Some(y_scale) => datamodel::GraphicalFunctionScale::from(y_scale),
            None => {
                let min = if y_points.is_empty() {
                    0.0
                } else {
                    y_points.iter().fold(f64::INFINITY, |a, &b| a.min(b))
                };
                let max = if y_points.is_empty() {
                    1.0
                } else {
                    y_points.iter().fold(-f64::INFINITY, |a, &b| a.max(b))
                };
                datamodel::GraphicalFunctionScale { min, max }
            }
        };

        datamodel::GraphicalFunction {
            kind,
            x_points: if x_points.is_empty() {
                None
            } else {
                Some(x_points)
            },
            y_points,
            x_scale,
            y_scale,
        }
    }
}

impl From<datamodel::GraphicalFunction> for GF {
    fn from(gf: datamodel::GraphicalFunction) -> Self {
        let x_pts: Option<String> = match gf.x_points {
            Some(x_points) => Some(
                x_points
                    .into_iter()
                    .map(|f| f.to_string())
                    .collect::<Vec<String>>()
                    .join(","),
            ),
            None => None,
        };
        let y_pts = gf
            .y_points
            .into_iter()
            .map(|f| f.to_string())
            .collect::<Vec<String>>()
            .join(",");
        GF {
            name: None,
            kind: Some(GraphicalFunctionKind::from(gf.kind)),
            x_scale: Some(GraphicalFunctionScale::from(gf.x_scale)),
            y_scale: Some(GraphicalFunctionScale::from(gf.y_scale)),
            x_pts,
            y_pts: Some(y_pts),
        }
    }
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

impl From<Model> for datamodel::Model {
    fn from(model: Model) -> Self {
        datamodel::Model {
            name: model.name.unwrap_or_else(|| "main".to_string()),
            variables: match model.variables {
                Some(vars) => vars
                    .variables
                    .into_iter()
                    .map(datamodel::Variable::from)
                    .collect(),
                None => vec![],
            },
            views: vec![],
        }
    }
}

impl From<datamodel::Model> for Model {
    fn from(model: datamodel::Model) -> Self {
        Model {
            name: Some(model.name),
            namespaces: None,
            resource: None,
            sim_specs: None,
            variables: if model.variables.is_empty() {
                None
            } else {
                let variables = model.variables.into_iter().map(Var::from).collect();
                Some(Variables { variables })
            },
            views: None,
        }
    }
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
    // Style(Style),
    #[serde(other)]
    Unhandled,
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
    pub model_name: Option<String>,
    pub doc: Option<String>,
    pub units: Option<String>,
    #[serde(rename = "$value", default)]
    pub refs: Vec<Reference>,
}

impl From<Module> for datamodel::Module {
    fn from(module: Module) -> Self {
        let ident = canonicalize(&module.name);
        // TODO: we should filter these to only module inputs, and rewrite
        //       the equations of variables that use module outputs
        let references: Vec<datamodel::ModuleReference> = module
            .refs
            .into_iter()
            .filter(|r| matches!(r, Reference::Connect(_)))
            .map(|r| {
                if let Reference::Connect(r) = r {
                    datamodel::ModuleReference {
                        src: canonicalize(&r.src),
                        dst: canonicalize(&r.dst),
                    }
                } else {
                    unreachable!();
                }
            })
            .collect();
        datamodel::Module {
            ident,
            model_name: match module.model_name {
                Some(model_name) => canonicalize(&model_name),
                None => canonicalize(&module.name),
            },
            documentation: module.doc.unwrap_or_default(),
            units: module.units,
            references,
        }
    }
}

impl From<datamodel::Module> for Module {
    fn from(module: datamodel::Module) -> Self {
        let refs: Vec<Reference> = module
            .references
            .into_iter()
            .map(|mi| {
                Reference::Connect(Connect {
                    src: mi.src,
                    dst: mi.dst,
                })
            })
            .collect();
        Module {
            name: module.ident,
            model_name: Some(module.model_name),
            doc: if module.documentation.is_empty() {
                None
            } else {
                Some(module.documentation)
            },
            units: module.units,
            refs,
        }
    }
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

impl From<Stock> for datamodel::Stock {
    fn from(stock: Stock) -> Self {
        let inflows = stock
            .inflows
            .unwrap_or_default()
            .into_iter()
            .map(|id| canonicalize(&id))
            .collect();
        let outflows = stock
            .outflows
            .unwrap_or_default()
            .into_iter()
            .map(|id| canonicalize(&id))
            .collect();
        datamodel::Stock {
            ident: canonicalize(&stock.name),
            equation: stock.eqn.unwrap_or_default(),
            documentation: stock.doc.unwrap_or_default(),
            units: stock.units,
            inflows,
            outflows,
            non_negative: stock.non_negative.is_some(),
        }
    }
}

impl From<datamodel::Stock> for Stock {
    fn from(stock: datamodel::Stock) -> Self {
        Stock {
            name: stock.ident,
            eqn: if stock.equation.is_empty() {
                None
            } else {
                Some(stock.equation)
            },
            doc: if stock.documentation.is_empty() {
                None
            } else {
                Some(stock.documentation)
            },
            units: stock.units,
            inflows: if stock.inflows.is_empty() {
                None
            } else {
                Some(stock.inflows)
            },
            outflows: if stock.outflows.is_empty() {
                None
            } else {
                Some(stock.outflows)
            },
            non_negative: if stock.non_negative {
                Some(NonNegative {})
            } else {
                None
            },
            dimensions: None,
        }
    }
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

impl From<Flow> for datamodel::Flow {
    fn from(flow: Flow) -> Self {
        datamodel::Flow {
            ident: canonicalize(&flow.name),
            equation: flow.eqn.unwrap_or_default(),
            documentation: flow.doc.unwrap_or_default(),
            units: flow.units,
            gf: match flow.gf {
                Some(gf) => Some(datamodel::GraphicalFunction::from(gf)),
                None => None,
            },
            non_negative: flow.non_negative.is_some(),
        }
    }
}

impl From<datamodel::Flow> for Flow {
    fn from(flow: datamodel::Flow) -> Self {
        Flow {
            name: flow.ident,
            eqn: if flow.equation.is_empty() {
                None
            } else {
                Some(flow.equation)
            },
            doc: if flow.documentation.is_empty() {
                None
            } else {
                Some(flow.documentation)
            },
            units: flow.units,
            gf: match flow.gf {
                Some(gf) => Some(GF::from(gf)),
                None => None,
            },
            non_negative: if flow.non_negative {
                Some(NonNegative {})
            } else {
                None
            },
            dimensions: None,
        }
    }
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

impl From<Aux> for datamodel::Aux {
    fn from(aux: Aux) -> Self {
        datamodel::Aux {
            ident: canonicalize(&aux.name),
            equation: aux.eqn.unwrap_or_default(),
            documentation: aux.doc.unwrap_or_default(),
            units: aux.units,
            gf: match aux.gf {
                Some(gf) => Some(datamodel::GraphicalFunction::from(gf)),
                None => None,
            },
        }
    }
}

impl From<datamodel::Aux> for Aux {
    fn from(aux: datamodel::Aux) -> Self {
        Aux {
            name: aux.ident,
            eqn: if aux.equation.is_empty() {
                None
            } else {
                Some(aux.equation)
            },
            doc: if aux.documentation.is_empty() {
                None
            } else {
                Some(aux.documentation)
            },
            units: aux.units,
            gf: match aux.gf {
                Some(gf) => Some(GF::from(gf)),
                None => None,
            },
            dimensions: None,
        }
    }
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

impl From<Var> for datamodel::Variable {
    fn from(var: Var) -> Self {
        match var {
            Var::Stock(stock) => datamodel::Variable::Stock(datamodel::Stock::from(stock)),
            Var::Flow(flow) => datamodel::Variable::Flow(datamodel::Flow::from(flow)),
            Var::Aux(aux) => datamodel::Variable::Aux(datamodel::Aux::from(aux)),
            Var::Module(module) => datamodel::Variable::Module(datamodel::Module::from(module)),
        }
    }
}

impl From<datamodel::Variable> for Var {
    fn from(var: datamodel::Variable) -> Self {
        match var {
            datamodel::Variable::Stock(stock) => Var::Stock(Stock::from(stock)),
            datamodel::Variable::Flow(flow) => Var::Flow(Flow::from(flow)),
            datamodel::Variable::Aux(aux) => Var::Aux(Aux::from(aux)),
            datamodel::Variable::Module(module) => Var::Module(Module::from(module)),
        }
    }
}

#[test]
fn test_canonicalize_stock_inflows() {
    use crate::common::canonicalize;

    let input = Var::Stock(Stock {
        name: canonicalize("Heat Loss To Room"),
        eqn: Some("total_population".to_string()),
        doc: Some("People who can contract the disease.".to_string()),
        units: Some("people".to_string()),
        inflows: Some(vec!["\"Solar Radiation\"".to_string()]),
        outflows: Some(vec![
            "\"succumbing\"".to_string(),
            "\"succumbing 2\"".to_string(),
        ]),
        non_negative: None,
        dimensions: None,
    });

    let expected = datamodel::Variable::Stock(datamodel::Stock {
        ident: "heat_loss_to_room".to_string(),
        equation: "total_population".to_string(),
        documentation: "People who can contract the disease.".to_string(),
        units: Some("people".to_string()),
        inflows: vec!["solar_radiation".to_string()],
        outflows: vec!["succumbing".to_string(), "succumbing_2".to_string()],
        non_negative: false,
    });

    let output = datamodel::Variable::from(input);

    assert_eq!(expected, output);
}

pub fn project_from_reader(reader: &mut dyn BufRead) -> Result<datamodel::Project> {
    use quick_xml::de;
    let file: File = match de::from_reader(reader) {
        Ok(file) => file,
        Err(err) => {
            return import_err!(XmlDeserialization, err.to_string());
        }
    };

    Ok(convert_file_to_project(&file))
}

pub fn convert_file_to_project(file: &File) -> datamodel::Project {
    datamodel::Project::from(file.clone())
}

#[test]
fn test_bad_xml() {
    let input = "<stock name=\"susceptible\">
        <eqn>total_population</eqn>
        <outflow>succumbing</outflow>
        <outflow>succumbing_2";

    use quick_xml::de;
    let stock: std::result::Result<Var, _> = de::from_reader(input.as_bytes());

    assert!(stock.is_err());
}

#[test]
fn test_xml_stock_parsing() {
    let input = "<stock name=\"susceptible\">
        <eqn>total_population</eqn>
        <outflow>succumbing</outflow>
        <outflow>succumbing_2</outflow>
        <doc>People who can contract the disease.</doc>
        <units>people</units>
    </stock>";

    let expected = Stock {
        name: "susceptible".to_string(),
        eqn: Some("total_population".to_string()),
        doc: Some("People who can contract the disease.".to_string()),
        units: Some("people".to_string()),
        inflows: None,
        outflows: Some(vec!["succumbing".to_string(), "succumbing_2".to_string()]),
        non_negative: None,
        dimensions: None,
    };

    use quick_xml::de;
    let stock: Var = de::from_reader(input.as_bytes()).unwrap();

    if let Var::Stock(stock) = stock {
        assert_eq!(expected, stock);
    } else {
        assert!(false);
    }
}

#[test]
fn test_xml_gf_parsing() {
    let input = "            <aux name=\"lookup function table\">
                <eqn>0</eqn>
                <gf>
                    <yscale min=\"-1\" max=\"1\"/>
                    <xpts>0,5,10,15,20,25,30,35,40,45</xpts>
                    <ypts>0,0,1,1,0,0,-1,-1,0,0</ypts>
                </gf>
            </aux>";

    let expected = Aux {
        name: "lookup function table".to_string(),
        eqn: Some("0".to_string()),
        doc: None,
        units: None,
        gf: Some(GF {
            name: None,
            kind: None,
            x_scale: None,
            y_scale: Some(GraphicalFunctionScale {
                min: -1.0,
                max: 1.0,
            }),
            x_pts: Some("0,5,10,15,20,25,30,35,40,45".to_string()),
            y_pts: Some("0,0,1,1,0,0,-1,-1,0,0".to_string()),
        }),
        dimensions: None,
    };

    use quick_xml::de;
    let aux: Var = de::from_reader(input.as_bytes()).unwrap();

    if let Var::Aux(aux) = aux {
        assert_eq!(expected, aux);
    } else {
        assert!(false);
    }
}

#[test]
fn test_module_parsing() {
    let input = "<module name=\"hares\" isee:label=\"\">
				<connect to=\"hares.area\" from=\".area\"/>
				<connect2 to=\"hares.area\" from=\"area\"/>
				<connect to=\"lynxes.hare_density\" from=\"hares.hare_density\"/>
				<connect2 to=\"lynxes.hare_density\" from=\"hares.hare_density\"/>
				<connect to=\"hares.lynxes\" from=\"lynxes.lynxes\"/>
				<connect2 to=\"hares.lynxes\" from=\"lynxes.lynxes\"/>
			</module>";

    let expected = Module {
        name: "hares".to_string(),
        model_name: None,
        doc: None,
        units: None,
        refs: vec![
            Reference::Connect(Connect {
                src: ".area".to_string(),
                dst: "hares.area".to_string(),
            }),
            Reference::Connect2(Connect {
                src: "area".to_string(),
                dst: "hares.area".to_string(),
            }),
            Reference::Connect(Connect {
                src: "hares.hare_density".to_string(),
                dst: "lynxes.hare_density".to_string(),
            }),
            Reference::Connect2(Connect {
                src: "hares.hare_density".to_string(),
                dst: "lynxes.hare_density".to_string(),
            }),
            Reference::Connect(Connect {
                src: "lynxes.lynxes".to_string(),
                dst: "hares.lynxes".to_string(),
            }),
            Reference::Connect2(Connect {
                src: "lynxes.lynxes".to_string(),
                dst: "hares.lynxes".to_string(),
            }),
        ],
    };

    use quick_xml::de;
    let actual: Module = de::from_reader(input.as_bytes()).unwrap();
    assert_eq!(expected, actual);

    let expected_roundtripped = Module {
        name: "hares".to_string(),
        model_name: Some("hares".to_string()),
        doc: None,
        units: None,
        refs: vec![
            Reference::Connect(Connect {
                src: ".area".to_string(),
                dst: "hares.area".to_string(),
            }),
            Reference::Connect(Connect {
                src: "hares.hare_density".to_string(),
                dst: "lynxes.hare_density".to_string(),
            }),
            Reference::Connect(Connect {
                src: "lynxes.lynxes".to_string(),
                dst: "hares.lynxes".to_string(),
            }),
        ],
    };

    let roundtripped = Module::from(datamodel::Module::from(actual.clone()));
    assert_eq!(expected_roundtripped, roundtripped);
}

#[test]
fn test_sim_specs_parsing() {
    let input = "<sim_specs method=\"euler\" time_units=\"Time\">
		<start>0</start>
		<stop>100</stop>
		<savestep>1</savestep>
		<dt>0.03125</dt>
	</sim_specs>";

    let expected = SimSpecs {
        start: 0.0,
        stop: 100.0,
        dt: Some(Dt {
            value: 0.03125,
            reciprocal: None,
        }),
        save_step: Some(1.0),
        method: Some("euler".to_string()),
        time_units: Some("Time".to_string()),
    };

    use quick_xml::de;
    let actual: SimSpecs = de::from_reader(input.as_bytes()).unwrap();
    assert_eq!(expected, actual);

    let roundtripped = SimSpecs::from(datamodel::SimSpecs::from(actual.clone()));
    assert_eq!(roundtripped, actual);
}
