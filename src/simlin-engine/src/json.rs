// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! JSON serialization for system dynamics models.
//!
//! Provides JSON format matching the Go `sd` package, with separate
//! arrays for stocks/flows/auxiliaries/modules rather than unified enums.
//!
//! # Example
//! ```no_run
//! use simlin_engine::json;
//!
//! let json_str = r#"{"name": "test", "sim_specs": {...}, ...}"#;
//! let json_proj: json::Project = serde_json::from_str(json_str)?;
//! let datamodel_proj: simlin_engine::datamodel::Project = json_proj.into();
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use serde::{Deserialize, Serialize};

use crate::canonicalize;
use crate::datamodel;

// Helper functions for serde skip_serializing_if

fn is_zero_i32(val: &i32) -> bool {
    *val == 0
}

fn is_zero_f64(val: &f64) -> bool {
    *val == 0.0
}

fn is_false(val: &bool) -> bool {
    !*val
}

fn is_empty_string(val: &str) -> bool {
    val.is_empty()
}

fn is_empty_vec<T>(val: &[T]) -> bool {
    val.is_empty()
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    T: Default + serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

// Type alias matching Go's Ident
pub type Ident = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElementEquation {
    pub subscript: String,
    pub equation: String,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub initial_equation: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub graphical_function: Option<GraphicalFunction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArrayedEquation {
    pub dimensions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub equation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub initial_equation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub elements: Option<Vec<ElementEquation>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphicalFunctionScale {
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphicalFunction {
    #[serde(skip_serializing_if = "is_empty_vec", default)]
    pub points: Vec<[f64; 2]>,
    #[serde(skip_serializing_if = "is_empty_vec", default)]
    pub y_points: Vec<f64>,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub x_scale: Option<GraphicalFunctionScale>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub y_scale: Option<GraphicalFunctionScale>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stock {
    #[serde(skip_serializing_if = "is_zero_i32", default)]
    pub uid: i32,
    pub name: String,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub initial_equation: String,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub units: String,
    pub inflows: Vec<Ident>,
    pub outflows: Vec<Ident>,
    #[serde(skip_serializing_if = "is_false", default)]
    pub non_negative: bool,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub documentation: String,
    #[serde(skip_serializing_if = "is_false", default)]
    pub can_be_module_input: bool,
    #[serde(skip_serializing_if = "is_false", default)]
    pub is_public: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arrayed_equation: Option<ArrayedEquation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Flow {
    #[serde(skip_serializing_if = "is_zero_i32", default)]
    pub uid: i32,
    pub name: String,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub equation: String,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub units: String,
    #[serde(skip_serializing_if = "is_false", default)]
    pub non_negative: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub graphical_function: Option<GraphicalFunction>,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub documentation: String,
    #[serde(skip_serializing_if = "is_false", default)]
    pub can_be_module_input: bool,
    #[serde(skip_serializing_if = "is_false", default)]
    pub is_public: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arrayed_equation: Option<ArrayedEquation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Auxiliary {
    #[serde(skip_serializing_if = "is_zero_i32", default)]
    pub uid: i32,
    pub name: String,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub equation: String,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub initial_equation: String,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub units: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub graphical_function: Option<GraphicalFunction>,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub documentation: String,
    #[serde(skip_serializing_if = "is_false", default)]
    pub can_be_module_input: bool,
    #[serde(skip_serializing_if = "is_false", default)]
    pub is_public: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arrayed_equation: Option<ArrayedEquation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleReference {
    pub src: String,
    pub dst: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Module {
    #[serde(skip_serializing_if = "is_zero_i32", default)]
    pub uid: i32,
    pub name: String,
    pub model_name: String,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub units: String,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub documentation: String,
    #[serde(skip_serializing_if = "is_empty_vec", default)]
    pub references: Vec<ModuleReference>,
    #[serde(skip_serializing_if = "is_false", default)]
    pub can_be_module_input: bool,
    #[serde(skip_serializing_if = "is_false", default)]
    pub is_public: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimSpecs {
    pub start_time: f64,
    pub end_time: f64,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub dt: String,
    #[serde(skip_serializing_if = "is_zero_f64", default)]
    pub save_step: f64,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub method: String,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub time_units: String,
}

// View element types

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowPoint {
    pub x: f64,
    pub y: f64,
    #[serde(skip_serializing_if = "is_zero_i32", default)]
    pub attached_to_uid: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StockViewElement {
    pub uid: i32,
    pub name: String,
    pub x: f64,
    pub y: f64,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub label_side: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowViewElement {
    pub uid: i32,
    pub name: String,
    pub x: f64,
    pub y: f64,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub label_side: String,
    pub points: Vec<FlowPoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuxiliaryViewElement {
    pub uid: i32,
    pub name: String,
    pub x: f64,
    pub y: f64,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub label_side: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CloudViewElement {
    pub uid: i32,
    pub flow_uid: i32,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkViewElement {
    pub uid: i32,
    pub from_uid: i32,
    pub to_uid: i32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arc: Option<f64>,
    #[serde(skip_serializing_if = "is_empty_vec", default)]
    pub multi_points: Vec<LinkPoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleViewElement {
    pub uid: i32,
    pub name: String,
    pub x: f64,
    pub y: f64,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub label_side: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AliasViewElement {
    pub uid: i32,
    pub alias_of_uid: i32,
    pub x: f64,
    pub y: f64,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub label_side: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ViewElement {
    Stock(StockViewElement),
    Flow(FlowViewElement),
    #[serde(rename = "aux")]
    Auxiliary(AuxiliaryViewElement),
    Cloud(CloudViewElement),
    Link(LinkViewElement),
    Module(ModuleViewElement),
    Alias(AliasViewElement),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct View {
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub kind: String,
    pub elements: Vec<ViewElement>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub view_box: Option<Rect>,
    #[serde(skip_serializing_if = "is_zero_f64", default)]
    pub zoom: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub name: String,
    pub stocks: Vec<Stock>,
    pub flows: Vec<Flow>,
    pub auxiliaries: Vec<Auxiliary>,
    #[serde(
        skip_serializing_if = "is_empty_vec",
        default,
        deserialize_with = "deserialize_null_default"
    )]
    pub modules: Vec<Module>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub sim_specs: Option<SimSpecs>,
    #[serde(skip_serializing_if = "is_empty_vec", default)]
    pub views: Vec<View>,
    #[serde(skip_serializing_if = "is_empty_vec", default)]
    pub loop_metadata: Vec<LoopMetadata>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dimension {
    pub name: String,
    #[serde(skip_serializing_if = "is_empty_vec", default)]
    pub elements: Vec<String>,
    #[serde(skip_serializing_if = "is_zero_i32", default)]
    pub size: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unit {
    pub name: String,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub equation: String,
    #[serde(skip_serializing_if = "is_false", default)]
    pub disabled: bool,
    #[serde(skip_serializing_if = "is_empty_vec", default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopMetadata {
    pub uids: Vec<i32>,
    #[serde(skip_serializing_if = "is_false", default)]
    pub deleted: bool,
    pub name: String,
    #[serde(skip_serializing_if = "is_empty_string", default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub sim_specs: SimSpecs,
    pub models: Vec<Model>,
    #[serde(skip_serializing_if = "is_empty_vec", default)]
    pub dimensions: Vec<Dimension>,
    #[serde(skip_serializing_if = "is_empty_vec", default)]
    pub units: Vec<Unit>,
}

// Conversions FROM json types TO datamodel types

impl From<GraphicalFunctionScale> for datamodel::GraphicalFunctionScale {
    fn from(scale: GraphicalFunctionScale) -> Self {
        datamodel::GraphicalFunctionScale {
            min: scale.min,
            max: scale.max,
        }
    }
}

impl From<GraphicalFunction> for datamodel::GraphicalFunction {
    fn from(gf: GraphicalFunction) -> Self {
        let kind = match gf.kind.as_str() {
            "discrete" => datamodel::GraphicalFunctionKind::Discrete,
            "extrapolate" => datamodel::GraphicalFunctionKind::Extrapolate,
            _ => datamodel::GraphicalFunctionKind::Continuous,
        };

        let x_points = if !gf.points.is_empty() {
            Some(gf.points.iter().map(|p| p[0]).collect())
        } else {
            None
        };

        let y_points = if !gf.points.is_empty() {
            gf.points.iter().map(|p| p[1]).collect()
        } else {
            gf.y_points
        };

        let x_scale = gf.x_scale.unwrap_or(GraphicalFunctionScale {
            min: 0.0,
            max: (y_points.len() - 1) as f64,
        });

        let y_scale = gf
            .y_scale
            .unwrap_or(GraphicalFunctionScale { min: 0.0, max: 1.0 });

        datamodel::GraphicalFunction {
            kind,
            x_points,
            y_points,
            x_scale: x_scale.into(),
            y_scale: y_scale.into(),
        }
    }
}

impl From<Stock> for datamodel::Stock {
    fn from(stock: Stock) -> Self {
        let equation = match stock.arrayed_equation {
            Some(arrayed) => {
                if let Some(elements) = arrayed.elements {
                    datamodel::Equation::Arrayed(
                        arrayed.dimensions,
                        elements
                            .into_iter()
                            .map(|ee| {
                                (
                                    ee.subscript,
                                    ee.equation,
                                    if ee.initial_equation.is_empty() {
                                        None
                                    } else {
                                        Some(ee.initial_equation)
                                    },
                                    ee.graphical_function.map(|gf| gf.into()),
                                )
                            })
                            .collect(),
                    )
                } else {
                    datamodel::Equation::ApplyToAll(
                        arrayed.dimensions,
                        arrayed.equation.unwrap_or_default(),
                        arrayed.initial_equation,
                    )
                }
            }
            None => datamodel::Equation::Scalar(stock.initial_equation, None),
        };

        datamodel::Stock {
            ident: stock.name,
            equation,
            documentation: stock.documentation,
            units: if stock.units.is_empty() {
                None
            } else {
                Some(stock.units)
            },
            inflows: stock.inflows,
            outflows: stock.outflows,
            non_negative: stock.non_negative,
            can_be_module_input: stock.can_be_module_input,
            visibility: if stock.is_public {
                datamodel::Visibility::Public
            } else {
                datamodel::Visibility::Private
            },
            ai_state: None,
            uid: if stock.uid == 0 {
                None
            } else {
                Some(stock.uid)
            },
        }
    }
}

impl From<Flow> for datamodel::Flow {
    fn from(flow: Flow) -> Self {
        let equation = match flow.arrayed_equation {
            Some(arrayed) => {
                if let Some(elements) = arrayed.elements {
                    datamodel::Equation::Arrayed(
                        arrayed.dimensions,
                        elements
                            .into_iter()
                            .map(|ee| {
                                (
                                    ee.subscript,
                                    ee.equation,
                                    if ee.initial_equation.is_empty() {
                                        None
                                    } else {
                                        Some(ee.initial_equation)
                                    },
                                    ee.graphical_function.map(|gf| gf.into()),
                                )
                            })
                            .collect(),
                    )
                } else {
                    datamodel::Equation::ApplyToAll(
                        arrayed.dimensions,
                        arrayed.equation.unwrap_or_default(),
                        arrayed.initial_equation,
                    )
                }
            }
            None => datamodel::Equation::Scalar(flow.equation, None),
        };

        datamodel::Flow {
            ident: flow.name,
            equation,
            documentation: flow.documentation,
            units: if flow.units.is_empty() {
                None
            } else {
                Some(flow.units)
            },
            gf: flow.graphical_function.map(|gf| gf.into()),
            non_negative: flow.non_negative,
            can_be_module_input: flow.can_be_module_input,
            visibility: if flow.is_public {
                datamodel::Visibility::Public
            } else {
                datamodel::Visibility::Private
            },
            ai_state: None,
            uid: if flow.uid == 0 { None } else { Some(flow.uid) },
        }
    }
}

impl From<Auxiliary> for datamodel::Aux {
    fn from(aux: Auxiliary) -> Self {
        let equation = match aux.arrayed_equation {
            Some(arrayed) => {
                if let Some(elements) = arrayed.elements {
                    datamodel::Equation::Arrayed(
                        arrayed.dimensions,
                        elements
                            .into_iter()
                            .map(|ee| {
                                (
                                    ee.subscript,
                                    ee.equation,
                                    if ee.initial_equation.is_empty() {
                                        None
                                    } else {
                                        Some(ee.initial_equation)
                                    },
                                    ee.graphical_function.map(|gf| gf.into()),
                                )
                            })
                            .collect(),
                    )
                } else {
                    datamodel::Equation::ApplyToAll(
                        arrayed.dimensions,
                        arrayed.equation.unwrap_or_default(),
                        arrayed.initial_equation,
                    )
                }
            }
            None => datamodel::Equation::Scalar(
                aux.equation,
                if aux.initial_equation.is_empty() {
                    None
                } else {
                    Some(aux.initial_equation)
                },
            ),
        };

        datamodel::Aux {
            ident: aux.name,
            equation,
            documentation: aux.documentation,
            units: if aux.units.is_empty() {
                None
            } else {
                Some(aux.units)
            },
            gf: aux.graphical_function.map(|gf| gf.into()),
            can_be_module_input: aux.can_be_module_input,
            visibility: if aux.is_public {
                datamodel::Visibility::Public
            } else {
                datamodel::Visibility::Private
            },
            ai_state: None,
            uid: if aux.uid == 0 { None } else { Some(aux.uid) },
        }
    }
}

impl From<ModuleReference> for datamodel::ModuleReference {
    fn from(mr: ModuleReference) -> Self {
        datamodel::ModuleReference {
            src: mr.src,
            dst: mr.dst,
        }
    }
}

impl From<Module> for datamodel::Module {
    fn from(module: Module) -> Self {
        datamodel::Module {
            ident: module.name,
            model_name: module.model_name,
            documentation: module.documentation,
            units: if module.units.is_empty() {
                None
            } else {
                Some(module.units)
            },
            references: module.references.into_iter().map(|r| r.into()).collect(),
            can_be_module_input: module.can_be_module_input,
            visibility: if module.is_public {
                datamodel::Visibility::Public
            } else {
                datamodel::Visibility::Private
            },
            ai_state: None,
            uid: if module.uid == 0 {
                None
            } else {
                Some(module.uid)
            },
        }
    }
}

impl From<SimSpecs> for datamodel::SimSpecs {
    fn from(ss: SimSpecs) -> Self {
        let dt = if ss.dt.is_empty() || ss.dt == "1" {
            datamodel::Dt::Dt(1.0)
        } else if ss.dt.starts_with("1/") {
            let reciprocal_str = &ss.dt[2..];
            let reciprocal_value = reciprocal_str.parse::<f64>().unwrap_or(1.0);
            datamodel::Dt::Reciprocal(reciprocal_value)
        } else {
            let dt_value = ss.dt.parse::<f64>().unwrap_or(1.0);
            datamodel::Dt::Dt(dt_value)
        };

        let save_step = if ss.save_step == 0.0 {
            None
        } else {
            Some(datamodel::Dt::Dt(ss.save_step))
        };

        let sim_method = match ss.method.as_str() {
            "rk4" => datamodel::SimMethod::RungeKutta4,
            _ => datamodel::SimMethod::Euler,
        };

        datamodel::SimSpecs {
            start: ss.start_time,
            stop: ss.end_time,
            dt,
            save_step,
            sim_method,
            time_units: if ss.time_units.is_empty() {
                None
            } else {
                Some(ss.time_units)
            },
        }
    }
}

impl From<ViewElement> for datamodel::ViewElement {
    fn from(ve: ViewElement) -> Self {
        match ve {
            ViewElement::Stock(s) => {
                datamodel::ViewElement::Stock(datamodel::view_element::Stock {
                    name: s.name,
                    uid: s.uid,
                    x: s.x,
                    y: s.y,
                    label_side: label_side_from_string(&s.label_side),
                })
            }
            ViewElement::Flow(f) => datamodel::ViewElement::Flow(datamodel::view_element::Flow {
                name: f.name,
                uid: f.uid,
                x: f.x,
                y: f.y,
                label_side: label_side_from_string(&f.label_side),
                points: f
                    .points
                    .into_iter()
                    .map(|p| datamodel::view_element::FlowPoint {
                        x: p.x,
                        y: p.y,
                        attached_to_uid: if p.attached_to_uid == 0 {
                            None
                        } else {
                            Some(p.attached_to_uid)
                        },
                    })
                    .collect(),
            }),
            ViewElement::Auxiliary(a) => {
                datamodel::ViewElement::Aux(datamodel::view_element::Aux {
                    name: a.name,
                    uid: a.uid,
                    x: a.x,
                    y: a.y,
                    label_side: label_side_from_string(&a.label_side),
                })
            }
            ViewElement::Cloud(c) => {
                datamodel::ViewElement::Cloud(datamodel::view_element::Cloud {
                    uid: c.uid,
                    flow_uid: c.flow_uid,
                    x: c.x,
                    y: c.y,
                })
            }
            ViewElement::Link(l) => datamodel::ViewElement::Link(datamodel::view_element::Link {
                uid: l.uid,
                from_uid: l.from_uid,
                to_uid: l.to_uid,
                shape: if let Some(arc) = l.arc {
                    datamodel::view_element::LinkShape::Arc(arc)
                } else if !l.multi_points.is_empty() {
                    datamodel::view_element::LinkShape::MultiPoint(
                        l.multi_points
                            .into_iter()
                            .map(|p| datamodel::view_element::FlowPoint {
                                x: p.x,
                                y: p.y,
                                attached_to_uid: None,
                            })
                            .collect(),
                    )
                } else {
                    datamodel::view_element::LinkShape::Straight
                },
            }),
            ViewElement::Module(m) => {
                datamodel::ViewElement::Module(datamodel::view_element::Module {
                    name: m.name,
                    uid: m.uid,
                    x: m.x,
                    y: m.y,
                    label_side: label_side_from_string(&m.label_side),
                })
            }
            ViewElement::Alias(a) => {
                datamodel::ViewElement::Alias(datamodel::view_element::Alias {
                    uid: a.uid,
                    alias_of_uid: a.alias_of_uid,
                    x: a.x,
                    y: a.y,
                    label_side: label_side_from_string(&a.label_side),
                })
            }
        }
    }
}

fn label_side_from_string(s: &str) -> datamodel::view_element::LabelSide {
    match s {
        "top" => datamodel::view_element::LabelSide::Top,
        "left" => datamodel::view_element::LabelSide::Left,
        "bottom" => datamodel::view_element::LabelSide::Bottom,
        "right" => datamodel::view_element::LabelSide::Right,
        _ => datamodel::view_element::LabelSide::Center,
    }
}

impl From<View> for datamodel::View {
    fn from(view: View) -> Self {
        datamodel::View::StockFlow(datamodel::StockFlow {
            elements: view.elements.into_iter().map(|e| e.into()).collect(),
            view_box: view
                .view_box
                .map(|vb| datamodel::Rect {
                    x: vb.x,
                    y: vb.y,
                    width: vb.width,
                    height: vb.height,
                })
                .unwrap_or_default(),
            zoom: if view.zoom == 0.0 { 1.0 } else { view.zoom },
        })
    }
}

impl From<Model> for datamodel::Model {
    fn from(model: Model) -> Self {
        let mut variables = Vec::new();

        for stock in model.stocks {
            variables.push(datamodel::Variable::Stock(stock.into()));
        }
        for flow in model.flows {
            variables.push(datamodel::Variable::Flow(flow.into()));
        }
        for aux in model.auxiliaries {
            variables.push(datamodel::Variable::Aux(aux.into()));
        }
        for module in model.modules {
            variables.push(datamodel::Variable::Module(module.into()));
        }

        datamodel::Model {
            name: model.name,
            sim_specs: model.sim_specs.map(|ss| ss.into()),
            variables,
            views: model.views.into_iter().map(|v| v.into()).collect(),
            loop_metadata: model
                .loop_metadata
                .into_iter()
                .map(|lm| lm.into())
                .collect(),
        }
    }
}

impl From<Dimension> for datamodel::Dimension {
    fn from(dim: Dimension) -> Self {
        if !dim.elements.is_empty() {
            datamodel::Dimension::Named(dim.name, dim.elements)
        } else if dim.size > 0 {
            datamodel::Dimension::Indexed(dim.name, dim.size as u32)
        } else {
            datamodel::Dimension::Named(dim.name, vec![])
        }
    }
}

impl From<Unit> for datamodel::Unit {
    fn from(unit: Unit) -> Self {
        datamodel::Unit {
            name: unit.name,
            equation: if unit.equation.is_empty() {
                None
            } else {
                Some(unit.equation)
            },
            disabled: unit.disabled,
            aliases: unit.aliases,
        }
    }
}

impl From<LoopMetadata> for datamodel::LoopMetadata {
    fn from(loop_metadata: LoopMetadata) -> Self {
        datamodel::LoopMetadata {
            uids: loop_metadata.uids,
            deleted: loop_metadata.deleted,
            name: loop_metadata.name,
            description: loop_metadata.description,
        }
    }
}

impl From<Project> for datamodel::Project {
    fn from(project: Project) -> Self {
        datamodel::Project {
            name: project.name,
            sim_specs: project.sim_specs.into(),
            dimensions: project.dimensions.into_iter().map(|d| d.into()).collect(),
            units: project.units.into_iter().map(|u| u.into()).collect(),
            models: project.models.into_iter().map(|m| m.into()).collect(),
            source: None,
            ai_information: None,
        }
    }
}

impl std::str::FromStr for Project {
    type Err = crate::common::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s).map_err(|err| {
            crate::common::Error::new(
                crate::common::ErrorKind::Import,
                crate::common::ErrorCode::Generic,
                Some(format!("Failed to parse JSON project: {}", err)),
            )
        })
    }
}

impl Project {
    /// Parse a Project from a reader
    pub fn from_reader(reader: impl std::io::Read) -> crate::common::Result<Self> {
        serde_json::from_reader(reader).map_err(|err| {
            crate::common::Error::new(
                crate::common::ErrorKind::Import,
                crate::common::ErrorCode::Generic,
                Some(format!("Failed to parse JSON project: {}", err)),
            )
        })
    }
}

// Conversions FROM datamodel types TO json types

impl From<datamodel::GraphicalFunctionScale> for GraphicalFunctionScale {
    fn from(scale: datamodel::GraphicalFunctionScale) -> Self {
        GraphicalFunctionScale {
            min: scale.min,
            max: scale.max,
        }
    }
}

impl From<datamodel::GraphicalFunction> for GraphicalFunction {
    fn from(gf: datamodel::GraphicalFunction) -> Self {
        let kind = match gf.kind {
            datamodel::GraphicalFunctionKind::Discrete => "discrete",
            datamodel::GraphicalFunctionKind::Extrapolate => "extrapolate",
            datamodel::GraphicalFunctionKind::Continuous => "continuous",
        }
        .to_string();

        let (points, y_points) = if let Some(x_points) = gf.x_points {
            let pts = x_points
                .into_iter()
                .zip(gf.y_points.iter())
                .map(|(x, y)| [x, *y])
                .collect();
            (pts, vec![])
        } else {
            (vec![], gf.y_points)
        };

        GraphicalFunction {
            points,
            y_points,
            kind,
            x_scale: Some(gf.x_scale.into()),
            y_scale: Some(gf.y_scale.into()),
        }
    }
}

impl From<datamodel::Stock> for Stock {
    fn from(stock: datamodel::Stock) -> Self {
        let (initial_equation, arrayed_equation) = match stock.equation {
            datamodel::Equation::Scalar(eq, init_eq) => (init_eq.unwrap_or(eq), None),
            datamodel::Equation::ApplyToAll(dims, eq, init_eq) => (
                String::new(),
                Some(ArrayedEquation {
                    dimensions: dims,
                    equation: Some(eq),
                    initial_equation: init_eq,
                    elements: None,
                }),
            ),
            datamodel::Equation::Arrayed(dims, elems) => {
                let ees = elems
                    .into_iter()
                    .map(
                        |(subscript, equation, initial_equation, gf)| ElementEquation {
                            subscript,
                            equation,
                            initial_equation: initial_equation.unwrap_or_default(),
                            graphical_function: gf.map(|g| g.into()),
                        },
                    )
                    .collect();
                (
                    String::new(),
                    Some(ArrayedEquation {
                        dimensions: dims,
                        equation: None,
                        initial_equation: None,
                        elements: Some(ees),
                    }),
                )
            }
        };

        Stock {
            uid: stock.uid.unwrap_or(0),
            name: stock.ident,
            initial_equation,
            units: stock.units.unwrap_or_default(),
            inflows: stock.inflows,
            outflows: stock.outflows,
            non_negative: stock.non_negative,
            documentation: stock.documentation,
            can_be_module_input: stock.can_be_module_input,
            is_public: matches!(stock.visibility, datamodel::Visibility::Public),
            arrayed_equation,
        }
    }
}

impl From<datamodel::Flow> for Flow {
    fn from(flow: datamodel::Flow) -> Self {
        let (equation, arrayed_equation) = match flow.equation {
            datamodel::Equation::Scalar(eq, _) => (eq, None),
            datamodel::Equation::ApplyToAll(dims, eq, init_eq) => (
                String::new(),
                Some(ArrayedEquation {
                    dimensions: dims,
                    equation: Some(eq),
                    initial_equation: init_eq,
                    elements: None,
                }),
            ),
            datamodel::Equation::Arrayed(dims, elems) => {
                let ees = elems
                    .into_iter()
                    .map(
                        |(subscript, equation, initial_equation, gf)| ElementEquation {
                            subscript,
                            equation,
                            initial_equation: initial_equation.unwrap_or_default(),
                            graphical_function: gf.map(|g| g.into()),
                        },
                    )
                    .collect();
                (
                    String::new(),
                    Some(ArrayedEquation {
                        dimensions: dims,
                        equation: None,
                        initial_equation: None,
                        elements: Some(ees),
                    }),
                )
            }
        };

        Flow {
            uid: flow.uid.unwrap_or(0),
            name: flow.ident,
            equation,
            units: flow.units.unwrap_or_default(),
            non_negative: flow.non_negative,
            graphical_function: flow.gf.map(|gf| gf.into()),
            documentation: flow.documentation,
            can_be_module_input: flow.can_be_module_input,
            is_public: matches!(flow.visibility, datamodel::Visibility::Public),
            arrayed_equation,
        }
    }
}

impl From<datamodel::Aux> for Auxiliary {
    fn from(aux: datamodel::Aux) -> Self {
        let (equation, initial_equation, arrayed_equation) = match aux.equation {
            datamodel::Equation::Scalar(eq, init_eq) => (eq, init_eq.unwrap_or_default(), None),
            datamodel::Equation::ApplyToAll(dims, eq, init_eq) => (
                String::new(),
                String::new(),
                Some(ArrayedEquation {
                    dimensions: dims,
                    equation: Some(eq),
                    initial_equation: init_eq,
                    elements: None,
                }),
            ),
            datamodel::Equation::Arrayed(dims, elems) => {
                let ees = elems
                    .into_iter()
                    .map(
                        |(subscript, equation, initial_equation, gf)| ElementEquation {
                            subscript,
                            equation,
                            initial_equation: initial_equation.unwrap_or_default(),
                            graphical_function: gf.map(|g| g.into()),
                        },
                    )
                    .collect();
                (
                    String::new(),
                    String::new(),
                    Some(ArrayedEquation {
                        dimensions: dims,
                        equation: None,
                        initial_equation: None,
                        elements: Some(ees),
                    }),
                )
            }
        };

        Auxiliary {
            uid: aux.uid.unwrap_or(0),
            name: aux.ident,
            equation,
            initial_equation,
            units: aux.units.unwrap_or_default(),
            graphical_function: aux.gf.map(|gf| gf.into()),
            documentation: aux.documentation,
            can_be_module_input: aux.can_be_module_input,
            is_public: matches!(aux.visibility, datamodel::Visibility::Public),
            arrayed_equation,
        }
    }
}

impl From<datamodel::ModuleReference> for ModuleReference {
    fn from(mr: datamodel::ModuleReference) -> Self {
        ModuleReference {
            src: mr.src,
            dst: mr.dst,
        }
    }
}

impl From<datamodel::Module> for Module {
    fn from(module: datamodel::Module) -> Self {
        Module {
            uid: module.uid.unwrap_or(0),
            name: module.ident,
            model_name: module.model_name,
            units: module.units.unwrap_or_default(),
            documentation: module.documentation,
            references: module.references.into_iter().map(|r| r.into()).collect(),
            can_be_module_input: module.can_be_module_input,
            is_public: matches!(module.visibility, datamodel::Visibility::Public),
        }
    }
}

impl From<datamodel::SimSpecs> for SimSpecs {
    fn from(ss: datamodel::SimSpecs) -> Self {
        let dt = match ss.dt {
            datamodel::Dt::Dt(v) if (v - 1.0).abs() < 1e-10 => String::new(),
            datamodel::Dt::Dt(v) if v.fract() == 0.0 => format!("{}", v as i64),
            datamodel::Dt::Dt(v) => format!("{}", v),
            datamodel::Dt::Reciprocal(v) if v.fract() == 0.0 => format!("1/{}", v as i64),
            datamodel::Dt::Reciprocal(v) => format!("1/{}", v),
        };

        let save_step = match ss.save_step {
            Some(datamodel::Dt::Dt(v)) => v,
            Some(datamodel::Dt::Reciprocal(v)) => 1.0 / v,
            None => 0.0,
        };

        let method = match ss.sim_method {
            datamodel::SimMethod::RungeKutta4 => "rk4".to_string(),
            datamodel::SimMethod::Euler => String::new(),
        };

        SimSpecs {
            start_time: ss.start,
            end_time: ss.stop,
            dt,
            save_step,
            method,
            time_units: ss.time_units.unwrap_or_default(),
        }
    }
}

fn label_side_to_string(ls: datamodel::view_element::LabelSide) -> String {
    match ls {
        datamodel::view_element::LabelSide::Top => "top",
        datamodel::view_element::LabelSide::Left => "left",
        datamodel::view_element::LabelSide::Bottom => "bottom",
        datamodel::view_element::LabelSide::Right => "right",
        datamodel::view_element::LabelSide::Center => "",
    }
    .to_string()
}

impl From<datamodel::ViewElement> for ViewElement {
    fn from(ve: datamodel::ViewElement) -> Self {
        match ve {
            datamodel::ViewElement::Stock(s) => ViewElement::Stock(StockViewElement {
                uid: s.uid,
                name: s.name,
                x: s.x,
                y: s.y,
                label_side: label_side_to_string(s.label_side),
            }),
            datamodel::ViewElement::Flow(f) => ViewElement::Flow(FlowViewElement {
                uid: f.uid,
                name: f.name,
                x: f.x,
                y: f.y,
                label_side: label_side_to_string(f.label_side),
                points: f
                    .points
                    .into_iter()
                    .map(|p| FlowPoint {
                        x: p.x,
                        y: p.y,
                        attached_to_uid: p.attached_to_uid.unwrap_or(0),
                    })
                    .collect(),
            }),
            datamodel::ViewElement::Aux(a) => ViewElement::Auxiliary(AuxiliaryViewElement {
                uid: a.uid,
                name: a.name,
                x: a.x,
                y: a.y,
                label_side: label_side_to_string(a.label_side),
            }),
            datamodel::ViewElement::Cloud(c) => ViewElement::Cloud(CloudViewElement {
                uid: c.uid,
                flow_uid: c.flow_uid,
                x: c.x,
                y: c.y,
            }),
            datamodel::ViewElement::Link(l) => ViewElement::Link(LinkViewElement {
                uid: l.uid,
                from_uid: l.from_uid,
                to_uid: l.to_uid,
                arc: match l.shape {
                    datamodel::view_element::LinkShape::Arc(arc) => Some(arc),
                    _ => None,
                },
                multi_points: match l.shape {
                    datamodel::view_element::LinkShape::MultiPoint(points) => points
                        .into_iter()
                        .map(|p| LinkPoint { x: p.x, y: p.y })
                        .collect(),
                    _ => vec![],
                },
            }),
            datamodel::ViewElement::Module(m) => ViewElement::Module(ModuleViewElement {
                uid: m.uid,
                name: m.name,
                x: m.x,
                y: m.y,
                label_side: label_side_to_string(m.label_side),
            }),
            datamodel::ViewElement::Alias(a) => ViewElement::Alias(AliasViewElement {
                uid: a.uid,
                alias_of_uid: a.alias_of_uid,
                x: a.x,
                y: a.y,
                label_side: label_side_to_string(a.label_side),
            }),
        }
    }
}

impl From<datamodel::View> for View {
    fn from(view: datamodel::View) -> Self {
        match view {
            datamodel::View::StockFlow(sf) => View {
                kind: "stock_flow".to_string(),
                elements: sf.elements.into_iter().map(|e| e.into()).collect(),
                view_box: Some(Rect {
                    x: sf.view_box.x,
                    y: sf.view_box.y,
                    width: sf.view_box.width,
                    height: sf.view_box.height,
                }),
                zoom: sf.zoom,
            },
        }
    }
}

impl From<datamodel::Model> for Model {
    fn from(model: datamodel::Model) -> Self {
        let mut stocks = Vec::new();
        let mut flows = Vec::new();
        let mut auxiliaries = Vec::new();
        let mut modules = Vec::new();

        for var in model.variables {
            match var {
                datamodel::Variable::Stock(s) => stocks.push(s.into()),
                datamodel::Variable::Flow(f) => flows.push(f.into()),
                datamodel::Variable::Aux(a) => auxiliaries.push(a.into()),
                datamodel::Variable::Module(m) => modules.push(m.into()),
            }
        }

        // Sort all arrays by canonical identifier for determinism
        stocks.sort_by_key(|s: &Stock| canonicalize(&s.name));
        flows.sort_by_key(|f: &Flow| canonicalize(&f.name));
        auxiliaries.sort_by_key(|a: &Auxiliary| canonicalize(&a.name));
        modules.sort_by_key(|m: &Module| canonicalize(&m.name));

        Model {
            name: model.name,
            stocks,
            flows,
            auxiliaries,
            modules,
            sim_specs: model.sim_specs.map(|ss| ss.into()),
            views: model.views.into_iter().map(|v| v.into()).collect(),
            loop_metadata: model
                .loop_metadata
                .into_iter()
                .map(|lm| lm.into())
                .collect(),
        }
    }
}

impl From<datamodel::Dimension> for Dimension {
    fn from(dim: datamodel::Dimension) -> Self {
        match dim {
            datamodel::Dimension::Named(name, elements) => Dimension {
                name,
                elements,
                size: 0,
            },
            datamodel::Dimension::Indexed(name, size) => Dimension {
                name,
                elements: vec![],
                size: size as i32,
            },
        }
    }
}

impl From<datamodel::Unit> for Unit {
    fn from(unit: datamodel::Unit) -> Self {
        Unit {
            name: unit.name,
            equation: unit.equation.unwrap_or_default(),
            disabled: unit.disabled,
            aliases: unit.aliases,
        }
    }
}

impl From<datamodel::LoopMetadata> for LoopMetadata {
    fn from(loop_metadata: datamodel::LoopMetadata) -> Self {
        LoopMetadata {
            uids: loop_metadata.uids,
            deleted: loop_metadata.deleted,
            name: loop_metadata.name,
            description: loop_metadata.description,
        }
    }
}

impl From<datamodel::Project> for Project {
    fn from(project: datamodel::Project) -> Self {
        Project {
            name: project.name,
            sim_specs: project.sim_specs.into(),
            models: project.models.into_iter().map(|m| m.into()).collect(),
            dimensions: project.dimensions.into_iter().map(|d| d.into()).collect(),
            units: project.units.into_iter().map(|u| u.into()).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graphical_function_roundtrip() {
        let cases = vec![
            (
                "explicit points continuous",
                GraphicalFunction {
                    points: vec![[0.0, 0.0], [1.0, 10.0], [2.0, 20.0]],
                    y_points: vec![],
                    kind: "continuous".to_string(),
                    x_scale: None,
                    y_scale: None,
                },
            ),
            (
                "explicit points with y_scale",
                GraphicalFunction {
                    points: vec![[0.0, 5.0], [10.0, 15.0], [20.0, 25.0]],
                    y_points: vec![],
                    kind: "discrete".to_string(),
                    x_scale: None,
                    y_scale: Some(GraphicalFunctionScale {
                        min: 5.0,
                        max: 25.0,
                    }),
                },
            ),
            (
                "scale-based with y_points",
                GraphicalFunction {
                    points: vec![],
                    y_points: vec![0.0, 0.0, 1.0, 1.0, 0.0, 0.0, -1.0, -1.0, 0.0, 0.0],
                    kind: "continuous".to_string(),
                    x_scale: Some(GraphicalFunctionScale {
                        min: 0.0,
                        max: 45.0,
                    }),
                    y_scale: None,
                },
            ),
            (
                "extrapolate function",
                GraphicalFunction {
                    points: vec![[-10.0, -5.0], [0.0, 0.0], [10.0, 5.0]],
                    y_points: vec![],
                    kind: "extrapolate".to_string(),
                    x_scale: None,
                    y_scale: None,
                },
            ),
        ];

        for (name, json_gf) in cases {
            // Convert to datamodel
            let dm_gf: datamodel::GraphicalFunction = json_gf.clone().into();

            // Convert back to JSON
            let json_gf2: GraphicalFunction = dm_gf.into();

            // Serialize and deserialize through JSON
            let json_str = serde_json::to_string(&json_gf2).unwrap();
            let json_gf3: GraphicalFunction = serde_json::from_str(&json_str).unwrap();

            // Verify roundtrip
            assert_eq!(json_gf.kind, json_gf3.kind, "Failed for: {}", name);
            if !json_gf.points.is_empty() {
                assert_eq!(
                    json_gf.points.len(),
                    json_gf3.points.len(),
                    "Failed for: {}",
                    name
                );
            } else {
                assert_eq!(json_gf.y_points, json_gf3.y_points, "Failed for: {}", name);
            }
        }
    }

    #[test]
    fn test_sim_specs_dt_roundtrip() {
        let cases = vec![
            ("empty DT", ""),
            ("regular decimal", "0.125"),
            ("reciprocal", "1/8"),
            ("integer", "1"),
            ("reciprocal with decimal", "1/32.5"),
        ];

        for (name, dt_str) in cases {
            let json_ss = SimSpecs {
                start_time: 0.0,
                end_time: 100.0,
                dt: dt_str.to_string(),
                save_step: 1.0,
                method: "euler".to_string(),
                time_units: "months".to_string(),
            };

            // Convert to datamodel
            let dm_ss: datamodel::SimSpecs = json_ss.clone().into();

            // Convert back to JSON
            let json_ss2: SimSpecs = dm_ss.into();

            // Serialize and deserialize
            let json_str = serde_json::to_string(&json_ss2).unwrap();
            let json_ss3: SimSpecs = serde_json::from_str(&json_str).unwrap();

            // Verify fields
            assert_eq!(
                json_ss.start_time, json_ss3.start_time,
                "Failed for: {}",
                name
            );
            assert_eq!(json_ss.end_time, json_ss3.end_time, "Failed for: {}", name);

            // DT value is preserved (empty and "1" both normalize to empty on roundtrip)
            if dt_str.is_empty() || dt_str == "1" {
                assert_eq!("", json_ss3.dt, "Failed for: {}", name);
            } else {
                assert_eq!(json_ss.dt, json_ss3.dt, "Failed for: {}", name);
            }
        }
    }

    #[test]
    fn test_stock_roundtrip() {
        let cases = vec![
            (
                "scalar stock",
                Stock {
                    uid: 42,
                    name: "population".to_string(),
                    initial_equation: "100".to_string(),
                    units: "people".to_string(),
                    inflows: vec!["births".to_string()],
                    outflows: vec!["deaths".to_string()],
                    non_negative: true,
                    documentation: "Total population".to_string(),
                    can_be_module_input: false,
                    is_public: true,
                    arrayed_equation: None,
                },
            ),
            (
                "apply to all stock",
                Stock {
                    uid: 7,
                    name: "inventory".to_string(),
                    initial_equation: String::new(),
                    units: String::new(),
                    inflows: vec!["production".to_string()],
                    outflows: vec!["sales".to_string()],
                    non_negative: false,
                    documentation: String::new(),
                    can_be_module_input: true,
                    is_public: false,
                    arrayed_equation: Some(ArrayedEquation {
                        dimensions: vec!["warehouses".to_string()],
                        equation: Some("50".to_string()),
                        initial_equation: None,
                        elements: None,
                    }),
                },
            ),
            (
                "arrayed stock",
                Stock {
                    uid: 0,
                    name: "inventory".to_string(),
                    initial_equation: String::new(),
                    units: String::new(),
                    inflows: vec![],
                    outflows: vec![],
                    non_negative: false,
                    documentation: String::new(),
                    can_be_module_input: true,
                    is_public: false,
                    arrayed_equation: Some(ArrayedEquation {
                        dimensions: vec!["cities".to_string()],
                        equation: None,
                        initial_equation: None,
                        elements: Some(vec![
                            ElementEquation {
                                subscript: "Boston".to_string(),
                                equation: "50".to_string(),
                                initial_equation: "10".to_string(),
                                graphical_function: None,
                            },
                            ElementEquation {
                                subscript: "NYC".to_string(),
                                equation: "100".to_string(),
                                initial_equation: String::new(),
                                graphical_function: None,
                            },
                        ]),
                    }),
                },
            ),
        ];

        for (name, json_stock) in cases {
            // Convert to datamodel
            let dm_stock: datamodel::Stock = json_stock.clone().into();

            // Convert back to JSON
            let json_stock2: Stock = dm_stock.into();

            // Serialize and deserialize
            let json_str = serde_json::to_string(&json_stock2).unwrap();
            let json_stock3: Stock = serde_json::from_str(&json_str).unwrap();

            // Verify core fields
            assert_eq!(json_stock.name, json_stock3.name, "Failed for: {}", name);
            assert_eq!(
                json_stock.inflows, json_stock3.inflows,
                "Failed for: {}",
                name
            );
            assert_eq!(
                json_stock.outflows, json_stock3.outflows,
                "Failed for: {}",
                name
            );
            assert_eq!(json_stock.units, json_stock3.units, "Failed for: {}", name);
            assert_eq!(
                json_stock.initial_equation, json_stock3.initial_equation,
                "Failed for: {}",
                name
            );
            assert_eq!(
                json_stock.is_public, json_stock3.is_public,
                "Failed for: {}",
                name
            );
            assert_eq!(
                json_stock.can_be_module_input, json_stock3.can_be_module_input,
                "Failed for: {}",
                name
            );
            assert_eq!(
                json_stock.arrayed_equation, json_stock3.arrayed_equation,
                "Failed for: {}",
                name
            );
        }
    }

    #[test]
    fn test_flow_roundtrip() {
        let cases = vec![
            (
                "scalar flow",
                Flow {
                    uid: 100,
                    name: "births".to_string(),
                    equation: "population * birth_rate".to_string(),
                    units: "people/year".to_string(),
                    non_negative: true,
                    graphical_function: Some(GraphicalFunction {
                        points: vec![[0.0, 0.0], [1.0, 1.0]],
                        y_points: vec![],
                        kind: "continuous".to_string(),
                        x_scale: Some(GraphicalFunctionScale { min: 0.0, max: 1.0 }),
                        y_scale: Some(GraphicalFunctionScale { min: 0.0, max: 1.0 }),
                    }),
                    documentation: "Birth flow".to_string(),
                    can_be_module_input: false,
                    is_public: true,
                    arrayed_equation: None,
                },
            ),
            (
                "apply to all flow",
                Flow {
                    uid: 0,
                    name: "production".to_string(),
                    equation: String::new(),
                    units: String::new(),
                    non_negative: false,
                    graphical_function: None,
                    documentation: String::new(),
                    can_be_module_input: true,
                    is_public: false,
                    arrayed_equation: Some(ArrayedEquation {
                        dimensions: vec!["factories".to_string()],
                        equation: Some("orders / lead_time".to_string()),
                        initial_equation: Some("initial_orders".to_string()),
                        elements: None,
                    }),
                },
            ),
            (
                "arrayed flow",
                Flow {
                    uid: 0,
                    name: "shipments".to_string(),
                    equation: String::new(),
                    units: String::new(),
                    non_negative: false,
                    graphical_function: None,
                    documentation: String::new(),
                    can_be_module_input: true,
                    is_public: false,
                    arrayed_equation: Some(ArrayedEquation {
                        dimensions: vec!["routes".to_string()],
                        equation: None,
                        initial_equation: None,
                        elements: Some(vec![
                            ElementEquation {
                                subscript: "east".to_string(),
                                equation: "supply_east".to_string(),
                                initial_equation: "init_supply_east".to_string(),
                                graphical_function: None,
                            },
                            ElementEquation {
                                subscript: "west".to_string(),
                                equation: "supply_west".to_string(),
                                initial_equation: String::new(),
                                graphical_function: None,
                            },
                        ]),
                    }),
                },
            ),
        ];

        for (name, json_flow) in cases {
            let dm_flow: datamodel::Flow = json_flow.clone().into();
            let json_flow2: Flow = dm_flow.into();
            let json_str = serde_json::to_string(&json_flow2).unwrap();
            let json_flow3: Flow = serde_json::from_str(&json_str).unwrap();

            assert_eq!(json_flow.name, json_flow3.name, "Failed for: {}", name);
            assert_eq!(
                json_flow.equation, json_flow3.equation,
                "Failed for: {}",
                name
            );
            assert_eq!(json_flow.units, json_flow3.units, "Failed for: {}", name);
            assert_eq!(
                json_flow.graphical_function, json_flow3.graphical_function,
                "Failed for: {}",
                name
            );
            assert_eq!(
                json_flow.non_negative, json_flow3.non_negative,
                "Failed for: {}",
                name
            );
            assert_eq!(
                json_flow.can_be_module_input, json_flow3.can_be_module_input,
                "Failed for: {}",
                name
            );
            assert_eq!(
                json_flow.is_public, json_flow3.is_public,
                "Failed for: {}",
                name
            );
            assert_eq!(
                json_flow.arrayed_equation, json_flow3.arrayed_equation,
                "Failed for: {}",
                name
            );
        }
    }

    #[test]
    fn test_auxiliary_roundtrip() {
        let cases = vec![
            (
                "scalar aux",
                Auxiliary {
                    uid: 200,
                    name: "birth_rate".to_string(),
                    equation: "0.02".to_string(),
                    initial_equation: "0.015".to_string(),
                    units: "1/year".to_string(),
                    graphical_function: None,
                    documentation: "Annual birth rate".to_string(),
                    can_be_module_input: true,
                    is_public: false,
                    arrayed_equation: None,
                },
            ),
            (
                "apply to all aux",
                Auxiliary {
                    uid: 0,
                    name: "capacity".to_string(),
                    equation: String::new(),
                    initial_equation: String::new(),
                    units: String::new(),
                    graphical_function: None,
                    documentation: String::new(),
                    can_be_module_input: false,
                    is_public: true,
                    arrayed_equation: Some(ArrayedEquation {
                        dimensions: vec!["plants".to_string()],
                        equation: Some("base_capacity".to_string()),
                        initial_equation: Some("initial_capacity".to_string()),
                        elements: None,
                    }),
                },
            ),
            (
                "arrayed aux",
                Auxiliary {
                    uid: 0,
                    name: "demand".to_string(),
                    equation: String::new(),
                    initial_equation: String::new(),
                    units: String::new(),
                    graphical_function: None,
                    documentation: String::new(),
                    can_be_module_input: false,
                    is_public: true,
                    arrayed_equation: Some(ArrayedEquation {
                        dimensions: vec!["regions".to_string()],
                        equation: None,
                        initial_equation: None,
                        elements: Some(vec![
                            ElementEquation {
                                subscript: "north".to_string(),
                                equation: "north_demand".to_string(),
                                initial_equation: "north_demand_init".to_string(),
                                graphical_function: None,
                            },
                            ElementEquation {
                                subscript: "south".to_string(),
                                equation: "south_demand".to_string(),
                                initial_equation: String::new(),
                                graphical_function: None,
                            },
                        ]),
                    }),
                },
            ),
        ];

        for (name, json_aux) in cases {
            let dm_aux: datamodel::Aux = json_aux.clone().into();
            let json_aux2: Auxiliary = dm_aux.into();
            let json_str = serde_json::to_string(&json_aux2).unwrap();
            let json_aux3: Auxiliary = serde_json::from_str(&json_str).unwrap();

            assert_eq!(json_aux.name, json_aux3.name, "Failed for: {}", name);
            assert_eq!(
                json_aux.equation, json_aux3.equation,
                "Failed for: {}",
                name
            );
            assert_eq!(
                json_aux.initial_equation, json_aux3.initial_equation,
                "Failed for: {}",
                name
            );
            assert_eq!(json_aux.units, json_aux3.units, "Failed for: {}", name);
            assert_eq!(
                json_aux.graphical_function, json_aux3.graphical_function,
                "Failed for: {}",
                name
            );
            assert_eq!(
                json_aux.can_be_module_input, json_aux3.can_be_module_input,
                "Failed for: {}",
                name
            );
            assert_eq!(
                json_aux.is_public, json_aux3.is_public,
                "Failed for: {}",
                name
            );
            assert_eq!(
                json_aux.arrayed_equation, json_aux3.arrayed_equation,
                "Failed for: {}",
                name
            );
        }
    }

    #[test]
    fn test_module_roundtrip() {
        let json_module = Module {
            uid: 300,
            name: "submodel".to_string(),
            model_name: "SubModel".to_string(),
            units: String::new(),
            documentation: "A submodel".to_string(),
            references: vec![ModuleReference {
                src: "input".to_string(),
                dst: "self.param".to_string(),
            }],
            can_be_module_input: false,
            is_public: true,
        };

        // Roundtrip
        let dm_module: datamodel::Module = json_module.clone().into();
        let json_module2: Module = dm_module.into();
        let json_str = serde_json::to_string(&json_module2).unwrap();
        let json_module3: Module = serde_json::from_str(&json_str).unwrap();

        assert_eq!(json_module.name, json_module3.name);
        assert_eq!(json_module.model_name, json_module3.model_name);
        assert_eq!(json_module.references.len(), json_module3.references.len());
    }

    #[test]
    fn test_view_element_roundtrip() {
        let cases = vec![
            (
                "stock",
                ViewElement::Stock(StockViewElement {
                    uid: 1,
                    name: "pop".to_string(),
                    x: 100.0,
                    y: 200.0,
                    label_side: "top".to_string(),
                }),
            ),
            (
                "flow",
                ViewElement::Flow(FlowViewElement {
                    uid: 2,
                    name: "rate".to_string(),
                    x: 150.0,
                    y: 200.0,
                    label_side: "left".to_string(),
                    points: vec![
                        FlowPoint {
                            x: 100.0,
                            y: 200.0,
                            attached_to_uid: 1,
                        },
                        FlowPoint {
                            x: 200.0,
                            y: 200.0,
                            attached_to_uid: 0,
                        },
                    ],
                }),
            ),
            (
                "cloud",
                ViewElement::Cloud(CloudViewElement {
                    uid: 3,
                    flow_uid: 2,
                    x: 50.0,
                    y: 200.0,
                }),
            ),
            (
                "link with arc",
                ViewElement::Link(LinkViewElement {
                    uid: 4,
                    from_uid: 1,
                    to_uid: 2,
                    arc: Some(45.0),
                    multi_points: vec![],
                }),
            ),
        ];

        for (name, json_ve) in cases {
            // Roundtrip through datamodel
            let dm_ve: datamodel::ViewElement = json_ve.clone().into();
            let json_ve2: ViewElement = dm_ve.into();

            // Serialize and deserialize
            let json_str = serde_json::to_string(&json_ve2).unwrap();
            let json_ve3: ViewElement = serde_json::from_str(&json_str).unwrap();

            // Verify type field is present in JSON
            assert!(
                json_str.contains("\"type\""),
                "Missing type field for: {}",
                name
            );

            // Verify basic structure matches
            match (&json_ve, &json_ve3) {
                (ViewElement::Stock(s1), ViewElement::Stock(s2)) => {
                    assert_eq!(s1.name, s2.name, "Failed for: {}", name);
                }
                (ViewElement::Flow(f1), ViewElement::Flow(f2)) => {
                    assert_eq!(f1.name, f2.name, "Failed for: {}", name);
                }
                (ViewElement::Cloud(c1), ViewElement::Cloud(c2)) => {
                    assert_eq!(c1.uid, c2.uid, "Failed for: {}", name);
                }
                (ViewElement::Link(l1), ViewElement::Link(l2)) => {
                    assert_eq!(l1.from_uid, l2.from_uid, "Failed for: {}", name);
                }
                _ => panic!("Type mismatch for: {}", name),
            }
        }
    }

    #[test]
    fn test_model_roundtrip() {
        let json_model = Model {
            name: "test_model".to_string(),
            stocks: vec![Stock {
                uid: 1,
                name: "stock1".to_string(),
                initial_equation: "100".to_string(),
                units: String::new(),
                inflows: vec!["flow1".to_string()],
                outflows: vec![],
                non_negative: false,
                documentation: String::new(),
                can_be_module_input: false,
                is_public: false,
                arrayed_equation: None,
            }],
            flows: vec![Flow {
                uid: 2,
                name: "flow1".to_string(),
                equation: "10".to_string(),
                units: String::new(),
                non_negative: false,
                graphical_function: None,
                documentation: String::new(),
                can_be_module_input: false,
                is_public: false,
                arrayed_equation: None,
            }],
            auxiliaries: vec![Auxiliary {
                uid: 3,
                name: "aux1".to_string(),
                equation: "5".to_string(),
                initial_equation: String::new(),
                units: String::new(),
                graphical_function: None,
                documentation: String::new(),
                can_be_module_input: false,
                is_public: false,
                arrayed_equation: None,
            }],
            modules: vec![],
            sim_specs: Some(SimSpecs {
                start_time: 5.0,
                end_time: 50.0,
                dt: "0.5".to_string(),
                save_step: 0.5,
                method: "rk4".to_string(),
                time_units: "Months".to_string(),
            }),
            views: vec![],
            loop_metadata: vec![],
        };

        // Roundtrip
        let dm_model: datamodel::Model = json_model.clone().into();
        let dm_model_specs = dm_model.sim_specs.clone();
        let json_model2: Model = dm_model.into();
        let json_str = serde_json::to_string(&json_model2).unwrap();
        let json_model3: Model = serde_json::from_str(&json_str).unwrap();

        // Verify structure
        assert_eq!(json_model.name, json_model3.name);
        assert_eq!(json_model.stocks.len(), json_model3.stocks.len());
        assert_eq!(json_model.flows.len(), json_model3.flows.len());
        assert_eq!(json_model.auxiliaries.len(), json_model3.auxiliaries.len());
        assert_eq!(dm_model_specs.as_ref().map(|ss| ss.start), Some(5.0));
        assert_eq!(
            json_model3.sim_specs.as_ref().map(|ss| (
                ss.start_time,
                ss.end_time,
                ss.dt.clone(),
                ss.method.clone(),
                ss.time_units.clone()
            )),
            Some((
                5.0,
                50.0,
                "0.5".to_string(),
                "rk4".to_string(),
                "Months".to_string()
            ))
        );

        // Verify arrays are sorted by canonical name
        assert_eq!(json_model3.stocks[0].name, "stock1");
        assert_eq!(json_model3.flows[0].name, "flow1");
        assert_eq!(json_model3.auxiliaries[0].name, "aux1");
    }

    #[test]
    fn test_model_without_sim_specs_defaults_to_none() {
        let json = r#"{
            "name": "test_model",
            "stocks": [],
            "flows": [],
            "auxiliaries": [],
            "modules": [],
            "views": [],
            "loop_metadata": []
        }"#;

        let json_model: Model = serde_json::from_str(json).unwrap();
        assert!(json_model.sim_specs.is_none());

        let dm_model: datamodel::Model = json_model.into();
        assert!(dm_model.sim_specs.is_none());

        let json_model2: Model = dm_model.into();
        assert!(json_model2.sim_specs.is_none());
    }

    #[test]
    fn test_project_roundtrip() {
        let json_project = Project {
            name: "test_project".to_string(),
            sim_specs: SimSpecs {
                start_time: 0.0,
                end_time: 100.0,
                dt: "0.25".to_string(),
                save_step: 1.0,
                method: "rk4".to_string(),
                time_units: "years".to_string(),
            },
            models: vec![Model {
                name: "main".to_string(),
                stocks: vec![],
                flows: vec![],
                auxiliaries: vec![],
                modules: vec![],
                sim_specs: Some(SimSpecs {
                    start_time: 0.0,
                    end_time: 100.0,
                    dt: "1".to_string(),
                    save_step: 1.0,
                    method: String::new(),
                    time_units: String::new(),
                }),
                views: vec![],
                loop_metadata: vec![],
            }],
            dimensions: vec![Dimension {
                name: "cities".to_string(),
                elements: vec!["Boston".to_string(), "NYC".to_string()],
                size: 0,
            }],
            units: vec![Unit {
                name: "people".to_string(),
                equation: String::new(),
                disabled: false,
                aliases: vec![],
            }],
        };

        // Roundtrip
        let dm_project: datamodel::Project = json_project.clone().into();
        let json_project2: Project = dm_project.into();
        let json_str = serde_json::to_string_pretty(&json_project2).unwrap();
        let json_project3: Project = serde_json::from_str(&json_str).unwrap();

        // Verify structure
        assert_eq!(json_project.name, json_project3.name);
        assert_eq!(json_project.models.len(), json_project3.models.len());
        assert_eq!(
            json_project.dimensions.len(),
            json_project3.dimensions.len()
        );
        assert_eq!(json_project.units.len(), json_project3.units.len());
        assert_eq!(
            json_project.sim_specs.method,
            json_project3.sim_specs.method
        );
        assert!(json_project3.models[0].sim_specs.is_some());
    }

    #[test]
    fn test_dimension_roundtrip() {
        let cases = vec![
            (
                "named dimension",
                Dimension {
                    name: "cities".to_string(),
                    elements: vec!["Boston".to_string(), "NYC".to_string()],
                    size: 0,
                },
            ),
            (
                "indexed dimension",
                Dimension {
                    name: "items".to_string(),
                    elements: vec![],
                    size: 10,
                },
            ),
        ];

        for (name, json_dim) in cases {
            let dm_dim: datamodel::Dimension = json_dim.clone().into();
            let json_dim2: Dimension = dm_dim.into();
            let json_str = serde_json::to_string(&json_dim2).unwrap();
            let json_dim3: Dimension = serde_json::from_str(&json_str).unwrap();

            assert_eq!(json_dim.name, json_dim3.name, "Failed for: {}", name);
            if json_dim.size > 0 {
                assert_eq!(json_dim.size, json_dim3.size, "Failed for: {}", name);
            } else {
                assert_eq!(
                    json_dim.elements, json_dim3.elements,
                    "Failed for: {}",
                    name
                );
            }
        }
    }

    #[test]
    fn test_deserialize_with_null_modules() {
        let json_str = r#"{
            "name": "test",
            "sim_specs": {
                "start_time": 0.0,
                "end_time": 10.0,
                "dt": "1",
                "method": "euler"
            },
            "models": [{
                "name": "main",
                "stocks": [],
                "flows": [],
                "auxiliaries": [],
                "modules": null,
                "sim_specs": {
                    "start_time": 0.0,
                    "end_time": 10.0,
                    "dt": "1",
                    "method": "euler"
                },
                "views": []
            }],
            "dimensions": [],
            "units": []
        }"#;

        let result: Result<Project, _> = serde_json::from_str(json_str);
        assert!(result.is_ok(), "Failed to deserialize: {:?}", result.err());
    }
}
