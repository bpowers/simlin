// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::io::BufRead;

use serde::{Deserialize, Serialize};

use std::collections::HashMap;
use system_dynamics_engine::common::{canonicalize, Result};
use system_dynamics_engine::datamodel;
use system_dynamics_engine::datamodel::{Equation, ViewElement};

macro_rules! import_err(
    ($code:tt, $str:expr) => {{
        use system_dynamics_engine::common::{Error, ErrorCode, ErrorKind};
        Err(Error::new(ErrorKind::Model, ErrorCode::$code, Some($str)))
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
            dimensions: match file.dimensions {
                None => vec![],
                Some(dimensions) => dimensions
                    .dimensions
                    .unwrap_or_default()
                    .into_iter()
                    .map(datamodel::Dimension::from)
                    .collect(),
            },
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
    #[serde(rename = "dim")]
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

impl From<Dimension> for datamodel::Dimension {
    fn from(dimension: Dimension) -> Self {
        datamodel::Dimension {
            name: dimension.name,
            elements: dimension
                .elements
                .unwrap_or_default()
                .into_iter()
                .map(|i| i.name)
                .collect(),
        }
    }
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
        let views = model
            .views
            .clone()
            .unwrap_or(Views { view: None })
            .view
            .unwrap_or_default()
            .into_iter()
            .filter(|v| v.kind.unwrap_or(ViewType::VendorSpecific) == ViewType::StockFlow)
            .map(|v| {
                let mut v = v;
                v.normalize(&model);
                datamodel::View::from(v)
            })
            .collect();
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
            views,
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
            views: if model.views.is_empty() {
                None
            } else {
                Some(Views {
                    view: Some(model.views.into_iter().map(View::from).collect()),
                })
            },
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

    // TODO: if this is a bottleneck, we should have a normalize pass over
    //   the model to canonicalize things once (and build a map)
    pub fn get_var(&self, ident: &str) -> Option<&Var> {
        self.variables.as_ref()?;

        for var in self.variables.as_ref().unwrap().variables.iter() {
            let name = var.get_noncanonical_name();
            if ident == name || ident == canonicalize(name) {
                return Some(var);
            }
        }

        None
    }
}

#[derive(Copy, Clone, PartialEq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewType {
    StockFlow,
    Interface,
    Popup,
    VendorSpecific,
}

pub mod view_element {
    use super::datamodel;
    use serde::{Deserialize, Serialize};
    use system_dynamics_engine::datamodel::view_element::LinkShape;

    #[derive(Copy, Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
    #[serde(rename_all = "snake_case")]
    pub enum LabelSide {
        Top,
        Left,
        Center,
        Bottom,
        Right,
    }

    impl From<LabelSide> for datamodel::view_element::LabelSide {
        fn from(label_side: LabelSide) -> Self {
            match label_side {
                LabelSide::Top => datamodel::view_element::LabelSide::Top,
                LabelSide::Left => datamodel::view_element::LabelSide::Left,
                LabelSide::Center => datamodel::view_element::LabelSide::Center,
                LabelSide::Bottom => datamodel::view_element::LabelSide::Bottom,
                LabelSide::Right => datamodel::view_element::LabelSide::Right,
            }
        }
    }

    impl From<datamodel::view_element::LabelSide> for LabelSide {
        fn from(label_side: datamodel::view_element::LabelSide) -> Self {
            match label_side {
                datamodel::view_element::LabelSide::Top => LabelSide::Top,
                datamodel::view_element::LabelSide::Left => LabelSide::Left,
                datamodel::view_element::LabelSide::Center => LabelSide::Center,
                datamodel::view_element::LabelSide::Bottom => LabelSide::Bottom,
                datamodel::view_element::LabelSide::Right => LabelSide::Right,
            }
        }
    }

    #[test]
    fn test_label_side_roundtrip() {
        let cases: &[_] = &[
            datamodel::view_element::LabelSide::Top,
            datamodel::view_element::LabelSide::Left,
            datamodel::view_element::LabelSide::Center,
            datamodel::view_element::LabelSide::Bottom,
            datamodel::view_element::LabelSide::Right,
        ];
        for expected in cases {
            let expected = expected.clone();
            let actual =
                datamodel::view_element::LabelSide::from(LabelSide::from(expected.clone()));
            assert_eq!(expected, actual);
        }
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Aux {
        pub name: String,
        pub uid: Option<i32>,
        pub x: f64,
        pub y: f64,
        pub width: Option<f64>,
        pub height: Option<f64>,
        pub label_side: Option<LabelSide>,
        pub label_angle: Option<f64>,
    }

    impl From<Aux> for datamodel::view_element::Aux {
        fn from(v: Aux) -> Self {
            datamodel::view_element::Aux {
                name: v.name,
                uid: v.uid.unwrap_or(-1),
                x: v.x,
                y: v.y,
                label_side: datamodel::view_element::LabelSide::from(
                    v.label_side.unwrap_or(LabelSide::Bottom),
                ),
            }
        }
    }

    impl From<datamodel::view_element::Aux> for Aux {
        fn from(v: datamodel::view_element::Aux) -> Self {
            Aux {
                name: v.name,
                uid: Some(v.uid),
                x: v.x,
                y: v.y,
                width: None,
                height: None,
                label_side: Some(LabelSide::from(v.label_side)),
                label_angle: None,
            }
        }
    }

    #[test]
    fn test_aux_roundtrip() {
        let cases: &[_] = &[datamodel::view_element::Aux {
            name: "test1".to_string(),
            uid: 32,
            x: 72.0,
            y: 28.0,
            label_side: datamodel::view_element::LabelSide::Top,
        }];
        for expected in cases {
            let expected = expected.clone();
            let actual = datamodel::view_element::Aux::from(Aux::from(expected.clone()));
            assert_eq!(expected, actual);
        }
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Stock {
        pub name: String,
        pub uid: Option<i32>,
        pub x: f64,
        pub y: f64,
        pub width: Option<f64>,
        pub height: Option<f64>,
        pub label_side: Option<LabelSide>,
        pub label_angle: Option<f64>,
    }

    impl From<Stock> for datamodel::view_element::Stock {
        fn from(v: Stock) -> Self {
            datamodel::view_element::Stock {
                name: v.name,
                uid: v.uid.unwrap_or(-1),
                x: v.x,
                y: v.y,
                label_side: datamodel::view_element::LabelSide::from(
                    v.label_side.unwrap_or(LabelSide::Bottom),
                ),
            }
        }
    }

    impl From<datamodel::view_element::Stock> for Stock {
        fn from(v: datamodel::view_element::Stock) -> Self {
            Stock {
                name: v.name,
                uid: Some(v.uid),
                x: v.x,
                y: v.y,
                width: None,
                height: None,
                label_side: Some(LabelSide::from(v.label_side)),
                label_angle: None,
            }
        }
    }

    #[test]
    fn test_stock_roundtrip() {
        let cases: &[_] = &[datamodel::view_element::Stock {
            name: "stock1".to_string(),
            uid: 33,
            x: 73.0,
            y: 29.0,
            label_side: datamodel::view_element::LabelSide::Center,
        }];
        for expected in cases {
            let expected = expected.clone();
            let actual = datamodel::view_element::Stock::from(Stock::from(expected.clone()));
            assert_eq!(expected, actual);
        }
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Point {
        pub x: f64,
        pub y: f64,
        pub uid: Option<i32>,
    }

    impl From<Point> for datamodel::view_element::FlowPoint {
        fn from(point: Point) -> Self {
            datamodel::view_element::FlowPoint {
                x: point.x,
                y: point.y,
                attached_to_uid: point.uid,
            }
        }
    }

    impl From<datamodel::view_element::FlowPoint> for Point {
        fn from(point: datamodel::view_element::FlowPoint) -> Self {
            Point {
                x: point.x,
                y: point.y,
                uid: point.attached_to_uid,
            }
        }
    }

    #[test]
    fn test_point_roundtrip() {
        let cases: &[_] = &[
            datamodel::view_element::FlowPoint {
                x: 1.1,
                y: 2.2,
                attached_to_uid: None,
            },
            datamodel::view_element::FlowPoint {
                x: 1.1,
                y: 2.2,
                attached_to_uid: Some(666),
            },
        ];
        for expected in cases {
            let expected = expected.clone();
            let actual = datamodel::view_element::FlowPoint::from(Point::from(expected.clone()));
            assert_eq!(expected, actual);
        }
    }

    #[derive(Clone, PartialEq, Debug, Default, Deserialize, Serialize)]
    pub struct Points {
        #[serde(rename = "pt")]
        pub points: Vec<Point>,
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Flow {
        pub name: String,
        pub uid: Option<i32>,
        pub x: f64,
        pub y: f64,
        pub width: Option<f64>,
        pub height: Option<f64>,
        pub label_side: Option<LabelSide>,
        pub label_angle: Option<f64>,
        #[serde(rename = "pts")]
        pub points: Option<Points>,
    }

    impl From<Flow> for datamodel::view_element::Flow {
        fn from(v: Flow) -> Self {
            datamodel::view_element::Flow {
                name: v.name,
                uid: v.uid.unwrap_or(-1),
                x: v.x,
                y: v.y,
                label_side: datamodel::view_element::LabelSide::from(
                    v.label_side.unwrap_or(LabelSide::Bottom),
                ),
                points: v
                    .points
                    .unwrap_or_default()
                    .points
                    .into_iter()
                    .map(datamodel::view_element::FlowPoint::from)
                    .collect(),
            }
        }
    }

    impl From<datamodel::view_element::Flow> for Flow {
        fn from(v: datamodel::view_element::Flow) -> Self {
            Flow {
                name: v.name,
                uid: Some(v.uid),
                x: v.x,
                y: v.y,
                width: None,
                height: None,
                label_side: Some(LabelSide::from(v.label_side)),
                label_angle: None,
                points: Some(Points {
                    points: v.points.into_iter().map(Point::from).collect(),
                }),
            }
        }
    }

    #[test]
    fn test_flow_roundtrip() {
        let cases: &[_] = &[datamodel::view_element::Flow {
            name: "inflow".to_string(),
            uid: 76,
            x: 1.1,
            y: 23.2,
            label_side: datamodel::view_element::LabelSide::Bottom,
            points: vec![
                datamodel::view_element::FlowPoint {
                    x: 1.1,
                    y: 2.2,
                    attached_to_uid: None,
                },
                datamodel::view_element::FlowPoint {
                    x: 1.1,
                    y: 2.2,
                    attached_to_uid: Some(666),
                },
            ],
        }];
        for expected in cases {
            let expected = expected.clone();
            let actual = datamodel::view_element::Flow::from(Flow::from(expected.clone()));
            assert_eq!(expected, actual);
        }
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Link {
        pub uid: Option<i32>,
        pub from: String,
        pub from_uid: Option<i32>,
        pub to: String,
        pub to_uid: Option<i32>,
        pub angle: Option<f64>,
        pub is_straight: Option<bool>,
        #[serde(rename = "pts")]
        pub points: Option<Points>, // for multi-point connectors
    }

    impl From<Link> for datamodel::view_element::Link {
        fn from(v: Link) -> Self {
            let shape = if v.is_straight.unwrap_or(false) {
                datamodel::view_element::LinkShape::Straight
            } else if v.points.is_some() {
                datamodel::view_element::LinkShape::MultiPoint(
                    v.points
                        .unwrap()
                        .points
                        .into_iter()
                        .map(datamodel::view_element::FlowPoint::from)
                        .collect(),
                )
            } else {
                datamodel::view_element::LinkShape::Arc(v.angle.unwrap_or(0.0))
            };
            datamodel::view_element::Link {
                uid: v.uid.unwrap_or(-1),
                from_uid: v.from_uid.unwrap_or(-1),
                to_uid: v.to_uid.unwrap_or(-1),
                shape,
            }
        }
    }

    impl From<datamodel::view_element::Link> for Link {
        fn from(v: datamodel::view_element::Link) -> Self {
            let (is_straight, angle, points) = match v.shape {
                LinkShape::Straight => (Some(true), None, None),
                LinkShape::Arc(angle) => (None, Some(angle), None),
                LinkShape::MultiPoint(points) => (
                    None,
                    None,
                    Some(Points {
                        points: points.into_iter().map(Point::from).collect(),
                    }),
                ),
            };
            Link {
                uid: Some(v.uid),
                from: "".to_string(),
                from_uid: Some(v.from_uid),
                to: "".to_string(),
                to_uid: Some(v.to_uid),
                angle,
                is_straight,
                points,
            }
        }
    }

    #[test]
    fn test_link_roundtrip() {
        let cases: &[_] = &[
            datamodel::view_element::Link {
                uid: 33,
                from_uid: 45,
                to_uid: 67,
                shape: LinkShape::Straight,
            },
            datamodel::view_element::Link {
                uid: 33,
                from_uid: 45,
                to_uid: 67,
                shape: LinkShape::Arc(351.3),
            },
            datamodel::view_element::Link {
                uid: 33,
                from_uid: 45,
                to_uid: 67,
                shape: LinkShape::MultiPoint(vec![datamodel::view_element::FlowPoint {
                    x: 1.1,
                    y: 2.2,
                    attached_to_uid: None,
                }]),
            },
        ];
        for expected in cases {
            let expected = expected.clone();
            let actual = datamodel::view_element::Link::from(Link::from(expected.clone()));
            assert_eq!(expected, actual);
        }
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Module {
        pub name: String,
        pub uid: Option<i32>,
        pub x: f64,
        pub y: f64,
        pub label_side: Option<LabelSide>,
    }

    impl From<Module> for datamodel::view_element::Module {
        fn from(v: Module) -> Self {
            datamodel::view_element::Module {
                name: v.name,
                uid: v.uid.unwrap_or(-1),
                x: v.x,
                y: v.y,
                label_side: datamodel::view_element::LabelSide::from(
                    v.label_side.unwrap_or(LabelSide::Bottom),
                ),
            }
        }
    }

    impl From<datamodel::view_element::Module> for Module {
        fn from(v: datamodel::view_element::Module) -> Self {
            Module {
                name: v.name,
                uid: Some(v.uid),
                x: v.x,
                y: v.y,
                label_side: Some(LabelSide::from(v.label_side)),
            }
        }
    }

    #[test]
    fn test_module_roundtrip() {
        let cases: &[_] = &[datamodel::view_element::Module {
            name: "stock1".to_string(),
            uid: 33,
            x: 73.0,
            y: 29.0,
            label_side: datamodel::view_element::LabelSide::Center,
        }];
        for expected in cases {
            let expected = expected.clone();
            let actual = datamodel::view_element::Module::from(Module::from(expected.clone()));
            assert_eq!(expected, actual);
        }
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Cloud {
        pub uid: i32,
        pub flow_uid: i32,
        pub x: f64,
        pub y: f64,
    }

    impl From<Cloud> for datamodel::view_element::Cloud {
        fn from(v: Cloud) -> Self {
            datamodel::view_element::Cloud {
                uid: v.uid,
                flow_uid: v.flow_uid,
                x: v.x,
                y: v.y,
            }
        }
    }

    impl From<datamodel::view_element::Cloud> for Cloud {
        fn from(v: datamodel::view_element::Cloud) -> Self {
            Cloud {
                uid: v.uid,
                flow_uid: v.flow_uid,
                x: v.x,
                y: v.y,
            }
        }
    }

    #[test]
    fn test_cloud_roundtrip() {
        let cases: &[_] = &[datamodel::view_element::Cloud {
            uid: 33,
            flow_uid: 31,
            x: 73.0,
            y: 29.0,
        }];
        for expected in cases {
            let expected = expected.clone();
            let actual = datamodel::view_element::Cloud::from(Cloud::from(expected.clone()));
            assert_eq!(expected, actual);
        }
    }
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewObject {
    Aux(view_element::Aux),
    Stock(view_element::Stock),
    Flow(view_element::Flow),
    #[serde(rename = "connector")]
    Link(view_element::Link),
    Module(view_element::Module),
    Cloud(view_element::Cloud),
    // Style(Style),
    #[serde(other)]
    Unhandled,
}

impl ViewObject {
    pub fn set_uid(&mut self, uid: i32) {
        match self {
            ViewObject::Aux(aux) => aux.uid = Some(uid),
            ViewObject::Stock(stock) => stock.uid = Some(uid),
            ViewObject::Flow(flow) => flow.uid = Some(uid),
            ViewObject::Link(link) => link.uid = Some(uid),
            ViewObject::Module(module) => module.uid = Some(uid),
            ViewObject::Cloud(cloud) => cloud.uid = uid,
            ViewObject::Unhandled => {}
        }
    }

    pub fn uid(&self) -> Option<i32> {
        match self {
            ViewObject::Aux(aux) => aux.uid,
            ViewObject::Stock(stock) => stock.uid,
            ViewObject::Flow(flow) => flow.uid,
            ViewObject::Link(link) => link.uid,
            ViewObject::Module(module) => module.uid,
            ViewObject::Cloud(cloud) => Some(cloud.uid),
            ViewObject::Unhandled => None,
        }
    }

    pub fn ident(&self) -> Option<String> {
        match self {
            ViewObject::Aux(aux) => Some(canonicalize(&aux.name)),
            ViewObject::Stock(stock) => Some(canonicalize(&stock.name)),
            ViewObject::Flow(flow) => Some(canonicalize(&flow.name)),
            ViewObject::Link(_link) => None,
            ViewObject::Module(module) => Some(canonicalize(&module.name)),
            ViewObject::Cloud(_cloud) => None,
            ViewObject::Unhandled => None,
        }
    }
}

impl From<ViewObject> for datamodel::ViewElement {
    fn from(v: ViewObject) -> Self {
        match v {
            ViewObject::Aux(v) => {
                datamodel::ViewElement::Aux(datamodel::view_element::Aux::from(v))
            }
            ViewObject::Stock(v) => {
                datamodel::ViewElement::Stock(datamodel::view_element::Stock::from(v))
            }
            ViewObject::Flow(v) => {
                datamodel::ViewElement::Flow(datamodel::view_element::Flow::from(v))
            }
            ViewObject::Link(v) => {
                datamodel::ViewElement::Link(datamodel::view_element::Link::from(v))
            }
            ViewObject::Module(v) => {
                datamodel::ViewElement::Module(datamodel::view_element::Module::from(v))
            }
            ViewObject::Cloud(v) => {
                datamodel::ViewElement::Cloud(datamodel::view_element::Cloud::from(v))
            }
            ViewObject::Unhandled => unreachable!("must filter out unhandled"),
        }
    }
}

impl From<datamodel::ViewElement> for ViewObject {
    fn from(v: datamodel::ViewElement) -> Self {
        match v {
            // TODO: rename ViewObject to ViewElement for consistency
            ViewElement::Aux(v) => ViewObject::Aux(view_element::Aux::from(v)),
            ViewElement::Stock(v) => ViewObject::Stock(view_element::Stock::from(v)),
            ViewElement::Flow(v) => ViewObject::Flow(view_element::Flow::from(v)),
            ViewElement::Link(v) => ViewObject::Link(view_element::Link::from(v)),
            ViewElement::Module(v) => ViewObject::Module(view_element::Module::from(v)),
            ViewElement::Alias(_v) => ViewObject::Unhandled, // TODO
            ViewElement::Cloud(_v) => ViewObject::Unhandled,
        }
    }
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct View {
    pub next_uid: Option<i32>, // used internally
    #[serde(rename = "type")]
    pub kind: Option<ViewType>,
    pub background: Option<String>,
    pub page_width: Option<String>,
    pub page_height: Option<String>,
    pub show_pages: Option<bool>,
    #[serde(rename = "$value", default)]
    pub objects: Vec<ViewObject>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CloudPosition {
    Source,
    Sink,
}

fn cloud_for(flow: &ViewObject, pos: CloudPosition, uid: i32) -> ViewObject {
    if let ViewObject::Flow(flow) = flow {
        let (x, y) = match pos {
            CloudPosition::Source => {
                let point = flow.points.as_ref().unwrap().points.first().unwrap();
                (point.x, point.y)
            }
            CloudPosition::Sink => {
                let point = flow.points.as_ref().unwrap().points.last().unwrap();
                (point.x, point.y)
            }
        };

        ViewObject::Cloud(view_element::Cloud {
            uid,
            flow_uid: flow.uid.unwrap(),
            x,
            y,
        })
    } else {
        unreachable!()
    }
}

impl View {
    fn assign_uids(&mut self) -> HashMap<String, i32> {
        let mut uid_map: HashMap<String, i32> = HashMap::new();
        let mut next_uid = 1;
        for o in self.objects.iter_mut() {
            o.set_uid(next_uid);
            if let Some(ident) = o.ident() {
                uid_map.insert(ident, next_uid);
            }
            next_uid += 1;
        }
        for o in self.objects.iter_mut() {
            if let ViewObject::Link(link) = o {
                link.from_uid = uid_map.get(&canonicalize(&link.from)).cloned();
                if link.from_uid == None {
                    panic!("unable to look up Link 'from' {}", link.from);
                }
                link.to_uid = uid_map.get(&canonicalize(&link.to)).cloned();
                if link.to_uid == None {
                    panic!("unable to look up Link 'to' {}", link.to);
                }
            }
        }

        self.next_uid = Some(next_uid);
        uid_map
    }

    fn get_flow_ends(
        &self,
        uid_map: &HashMap<String, i32>,
        model: &Model,
    ) -> HashMap<i32, (Option<i32>, Option<i32>)> {
        let display_stocks: Vec<&ViewObject> = self
            .objects
            .iter()
            .filter(|v| matches!(v, ViewObject::Stock(_)))
            .collect();
        let display_flows: Vec<&ViewObject> = self
            .objects
            .iter()
            .filter(|v| matches!(v, ViewObject::Flow(_)))
            .collect();
        let mut result: HashMap<i32, (Option<i32>, Option<i32>)> = display_flows
            .iter()
            .map(|v| (v.uid().unwrap(), (None, None)))
            .collect();

        for element in display_stocks {
            let ident = element.ident().unwrap();
            if let Var::Stock(stock) = model.get_var(&ident).unwrap() {
                if stock.outflows.is_some() {
                    for outflow in stock.outflows.as_ref().unwrap() {
                        let outflow_ident = canonicalize(outflow);
                        if !uid_map.contains_key(&outflow_ident) {
                            continue;
                        }
                        let outflow_uid = uid_map[&outflow_ident];
                        let end = result.get_mut(&outflow_uid).unwrap();
                        end.0 = Some(uid_map[&ident]);
                    }
                }
                if stock.inflows.is_some() {
                    for inflow in stock.inflows.as_ref().unwrap() {
                        let inflow_ident = canonicalize(inflow);
                        if !uid_map.contains_key(&inflow_ident) {
                            continue;
                        }
                        let inflow_uid = uid_map[&inflow_ident];
                        let end = result.get_mut(&inflow_uid).unwrap();
                        end.1 = Some(uid_map[&ident]);
                    }
                }
            }
        }

        result
    }

    fn fixup_clouds(&mut self, model: &Model, uid_map: &HashMap<String, i32>) {
        if model.variables.is_none() {
            // nothing to do if there are no variables
            return;
        }
        let flow_ends = self.get_flow_ends(uid_map, model);
        let mut clouds: Vec<ViewObject> = Vec::new();

        let display_flows: Vec<&mut ViewObject> = self
            .objects
            .iter_mut()
            .filter(|v| matches!(v, ViewObject::Flow(_)))
            .collect();

        for flow in display_flows {
            let ends = &flow_ends[&flow.uid().unwrap()];
            let source_uid = match ends.0 {
                None => {
                    let uid = self.next_uid.unwrap();
                    self.next_uid = Some(uid + 1);
                    let cloud = cloud_for(flow, CloudPosition::Source, uid);
                    clouds.push(cloud);
                    uid
                }
                Some(uid) => uid,
            };
            let sink_uid = match ends.1 {
                None => {
                    let uid = self.next_uid.unwrap();
                    self.next_uid = Some(uid + 1);
                    let cloud = cloud_for(flow, CloudPosition::Sink, uid);
                    clouds.push(cloud);
                    uid
                }
                Some(uid) => uid,
            };

            if let ViewObject::Flow(flow) = flow {
                if flow.points.is_some() && !flow.points.as_ref().unwrap().points.is_empty() {
                    let points = flow.points.as_mut().unwrap();
                    let source_point = points.points.first_mut().unwrap();
                    source_point.uid = Some(source_uid);
                    let sink_point = points.points.last_mut().unwrap();
                    sink_point.uid = Some(sink_uid);
                }
            } else {
                unreachable!()
            }
        }

        self.objects.append(&mut clouds);
    }

    fn normalize(&mut self, model: &Model) {
        if self.kind.unwrap_or(ViewType::VendorSpecific) != ViewType::StockFlow {
            return;
        }
        let uid_map = self.assign_uids();
        self.fixup_clouds(model, &uid_map);
    }
}

impl From<View> for datamodel::View {
    fn from(v: View) -> Self {
        if v.kind.unwrap_or(ViewType::VendorSpecific) == ViewType::StockFlow {
            datamodel::View::StockFlow(datamodel::StockFlow {
                elements: v
                    .objects
                    .into_iter()
                    .filter(|v| !matches!(v, ViewObject::Unhandled))
                    .map(datamodel::ViewElement::from)
                    .collect(),
            })
        } else {
            unreachable!("only stock_flow supported for now -- should be filtered out before here")
        }
    }
}

impl From<datamodel::View> for View {
    fn from(v: datamodel::View) -> Self {
        match v {
            datamodel::View::StockFlow(v) => View {
                next_uid: None,
                kind: Some(ViewType::StockFlow),
                background: None,
                page_width: None,
                page_height: None,
                show_pages: None,
                objects: v.elements.into_iter().map(ViewObject::from).collect(),
            },
        }
    }
}

#[test]
fn test_view_roundtrip() {
    let cases: &[_] = &[datamodel::View::StockFlow(datamodel::StockFlow {
        elements: vec![datamodel::ViewElement::Stock(
            datamodel::view_element::Stock {
                name: "stock1".to_string(),
                uid: 1,
                x: 73.0,
                y: 29.0,
                label_side: datamodel::view_element::LabelSide::Center,
            },
        )],
    })];
    for expected in cases {
        let expected = expected.clone();
        let actual = datamodel::View::from(View::from(expected.clone()));
        assert_eq!(expected, actual);
    }
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
pub struct VarElement {
    pub subscript: String,
    pub eqn: String,
}

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
    #[serde(rename = "element", default)]
    pub elements: Option<Vec<VarElement>>,
}

macro_rules! convert_equation(
    ($var:expr) => {{
        if let Some(elements) = $var.elements {
            let dimensions = match $var.dimensions {
                Some(dimensions) => dimensions.dimensions.unwrap().into_iter().map(|e| e.name).collect(),
                None => vec![],
            };
            let elements = elements.into_iter().map(|e| (e.subscript, e.eqn)).collect();
            datamodel::Equation::Arrayed(dimensions, elements)
        } else if let Some(dimensions) = $var.dimensions {
            let dimensions = dimensions.dimensions.unwrap_or_default().into_iter().map(|e| e.name).collect();
            datamodel::Equation::ApplyToAll(dimensions, $var.eqn.unwrap_or_default())
        } else {
            datamodel::Equation::Scalar($var.eqn.unwrap_or_default())
        }
    }}
);

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
            equation: convert_equation!(stock),
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
            eqn: match &stock.equation {
                Equation::Scalar(eqn) => {
                    if eqn.is_empty() {
                        None
                    } else {
                        Some(eqn.clone())
                    }
                }
                Equation::ApplyToAll(_, eqn) => {
                    if eqn.is_empty() {
                        None
                    } else {
                        Some(eqn.clone())
                    }
                }
                Equation::Arrayed(_, _) => None,
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
            dimensions: match &stock.equation {
                Equation::Scalar(_) => None,
                Equation::ApplyToAll(dims, _) => Some(VarDimensions {
                    dimensions: Some(
                        dims.iter()
                            .map(|name| VarDimension { name: name.clone() })
                            .collect(),
                    ),
                }),
                Equation::Arrayed(dims, _) => Some(VarDimensions {
                    dimensions: Some(
                        dims.iter()
                            .map(|name| VarDimension { name: name.clone() })
                            .collect(),
                    ),
                }),
            },
            elements: match stock.equation {
                Equation::Scalar(_) => None,
                Equation::ApplyToAll(_, _) => None,
                Equation::Arrayed(_, elements) => Some(
                    elements
                        .into_iter()
                        .map(|(subscript, eqn)| VarElement { subscript, eqn })
                        .collect(),
                ),
            },
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
    #[serde(rename = "element", default)]
    pub elements: Option<Vec<VarElement>>,
}

impl From<Flow> for datamodel::Flow {
    fn from(flow: Flow) -> Self {
        datamodel::Flow {
            ident: canonicalize(&flow.name),
            equation: convert_equation!(flow),
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
            eqn: match &flow.equation {
                Equation::Scalar(eqn) => {
                    if eqn.is_empty() {
                        None
                    } else {
                        Some(eqn.clone())
                    }
                }
                Equation::ApplyToAll(_, eqn) => {
                    if eqn.is_empty() {
                        None
                    } else {
                        Some(eqn.clone())
                    }
                }
                Equation::Arrayed(_, _) => None,
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
            dimensions: match &flow.equation {
                Equation::Scalar(_) => None,
                Equation::ApplyToAll(dims, _) => Some(VarDimensions {
                    dimensions: Some(
                        dims.iter()
                            .map(|name| VarDimension { name: name.clone() })
                            .collect(),
                    ),
                }),
                Equation::Arrayed(dims, _) => Some(VarDimensions {
                    dimensions: Some(
                        dims.iter()
                            .map(|name| VarDimension { name: name.clone() })
                            .collect(),
                    ),
                }),
            },
            elements: match flow.equation {
                Equation::Scalar(_) => None,
                Equation::ApplyToAll(_, _) => None,
                Equation::Arrayed(_, elements) => Some(
                    elements
                        .into_iter()
                        .map(|(subscript, eqn)| VarElement { subscript, eqn })
                        .collect(),
                ),
            },
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
    #[serde(rename = "element", default)]
    pub elements: Option<Vec<VarElement>>,
}

impl From<Aux> for datamodel::Aux {
    fn from(aux: Aux) -> Self {
        datamodel::Aux {
            ident: canonicalize(&aux.name),
            equation: convert_equation!(aux),
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
            eqn: match &aux.equation {
                Equation::Scalar(eqn) => {
                    if eqn.is_empty() {
                        None
                    } else {
                        Some(eqn.clone())
                    }
                }
                Equation::ApplyToAll(_, eqn) => {
                    if eqn.is_empty() {
                        None
                    } else {
                        Some(eqn.clone())
                    }
                }
                Equation::Arrayed(_, _) => None,
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
            dimensions: match &aux.equation {
                Equation::Scalar(_) => None,
                Equation::ApplyToAll(dims, _) => Some(VarDimensions {
                    dimensions: Some(
                        dims.iter()
                            .map(|name| VarDimension { name: name.clone() })
                            .collect(),
                    ),
                }),
                Equation::Arrayed(dims, _) => Some(VarDimensions {
                    dimensions: Some(
                        dims.iter()
                            .map(|name| VarDimension { name: name.clone() })
                            .collect(),
                    ),
                }),
            },
            elements: match aux.equation {
                Equation::Scalar(_) => None,
                Equation::ApplyToAll(_, _) => None,
                Equation::Arrayed(_, elements) => Some(
                    elements
                        .into_iter()
                        .map(|(subscript, eqn)| VarElement { subscript, eqn })
                        .collect(),
                ),
            },
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
    use system_dynamics_engine::common::canonicalize;

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
        elements: None,
    });

    let expected = datamodel::Variable::Stock(datamodel::Stock {
        ident: "heat_loss_to_room".to_string(),
        equation: Equation::Scalar("total_population".to_string()),
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
        elements: None,
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
        elements: None,
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
