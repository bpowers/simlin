// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! SDAI JSON serialization for system dynamics models.
//!
//! Provides JSON format for AI-generated system dynamics models,
//! with a flatter structure and discriminated union for variables.
//!
//! # Example
//! ```no_run
//! use simlin_engine::json_sdai;
//!
//! let json_str = r#"{"variables": [...], "specs": {...}}"#;
//! let sdai_model: json_sdai::SdaiModel = serde_json::from_str(json_str)?;
//! let datamodel_proj: simlin_engine::datamodel::Project = sdai_model.into();
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#[cfg(feature = "schema")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::datamodel;

fn is_none<T>(val: &Option<T>) -> bool {
    val.is_none()
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct GraphicalFunction {
    pub points: Vec<Point>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct StockFields {
    pub name: String,
    #[serde(skip_serializing_if = "is_none")]
    pub equation: Option<String>,
    #[serde(skip_serializing_if = "is_none")]
    pub documentation: Option<String>,
    #[serde(skip_serializing_if = "is_none")]
    pub units: Option<String>,
    #[serde(skip_serializing_if = "is_none")]
    pub inflows: Option<Vec<String>>,
    #[serde(skip_serializing_if = "is_none")]
    pub outflows: Option<Vec<String>>,
    #[serde(rename = "graphicalFunction", skip_serializing_if = "is_none")]
    #[cfg_attr(feature = "schema", schemars(rename = "graphicalFunction"))]
    pub graphical_function: Option<GraphicalFunction>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct FlowFields {
    pub name: String,
    #[serde(skip_serializing_if = "is_none")]
    pub equation: Option<String>,
    #[serde(skip_serializing_if = "is_none")]
    pub documentation: Option<String>,
    #[serde(skip_serializing_if = "is_none")]
    pub units: Option<String>,
    #[serde(rename = "graphicalFunction", skip_serializing_if = "is_none")]
    #[cfg_attr(feature = "schema", schemars(rename = "graphicalFunction"))]
    pub graphical_function: Option<GraphicalFunction>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct AuxiliaryFields {
    pub name: String,
    #[serde(skip_serializing_if = "is_none")]
    pub equation: Option<String>,
    #[serde(skip_serializing_if = "is_none")]
    pub documentation: Option<String>,
    #[serde(skip_serializing_if = "is_none")]
    pub units: Option<String>,
    #[serde(rename = "graphicalFunction", skip_serializing_if = "is_none")]
    #[cfg_attr(feature = "schema", schemars(rename = "graphicalFunction"))]
    pub graphical_function: Option<GraphicalFunction>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(tag = "type", rename_all = "lowercase")]
#[cfg_attr(feature = "schema", schemars(tag = "type", rename_all = "lowercase"))]
pub enum Variable {
    Stock(StockFields),
    Flow(FlowFields),
    Variable(AuxiliaryFields),
}

/// Polarity of a causal relationship in a system dynamics model.
/// Indicates whether an increase in the source variable causes an
/// increase (+), decrease (-), or unknown effect (?) on the target.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub enum Polarity {
    /// Positive polarity: increase in source causes increase in target
    #[serde(rename = "+")]
    Positive,
    /// Negative polarity: increase in source causes decrease in target
    #[serde(rename = "-")]
    Negative,
    /// Unknown polarity: relationship exists but direction of effect is unclear
    #[serde(rename = "?")]
    Unknown,
}

impl std::fmt::Display for Polarity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Polarity::Positive => write!(f, "+"),
            Polarity::Negative => write!(f, "-"),
            Polarity::Unknown => write!(f, "?"),
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct Relationship {
    #[serde(skip_serializing_if = "is_none")]
    pub reasoning: Option<String>,
    pub from: String,
    pub to: String,
    pub polarity: Polarity,
    #[serde(rename = "polarityReasoning", skip_serializing_if = "is_none")]
    #[cfg_attr(feature = "schema", schemars(rename = "polarityReasoning"))]
    pub polarity_reasoning: Option<String>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct SimSpecs {
    #[serde(rename = "startTime")]
    #[cfg_attr(feature = "schema", schemars(rename = "startTime"))]
    pub start_time: f64,
    #[serde(rename = "stopTime")]
    #[cfg_attr(feature = "schema", schemars(rename = "stopTime"))]
    pub stop_time: f64,
    #[serde(skip_serializing_if = "is_none")]
    pub dt: Option<f64>,
    #[serde(rename = "timeUnits", skip_serializing_if = "is_none")]
    #[cfg_attr(feature = "schema", schemars(rename = "timeUnits"))]
    pub time_units: Option<String>,
    #[serde(rename = "saveStep", skip_serializing_if = "is_none")]
    #[cfg_attr(feature = "schema", schemars(rename = "saveStep"))]
    pub save_step: Option<f64>,
    #[serde(skip_serializing_if = "is_none")]
    pub method: Option<String>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct SdaiModel {
    pub variables: Vec<Variable>,
    #[serde(skip_serializing_if = "is_none")]
    pub relationships: Option<Vec<Relationship>>,
    #[serde(skip_serializing_if = "is_none")]
    pub specs: Option<SimSpecs>,
    #[serde(skip_serializing_if = "is_none")]
    pub views: Option<Vec<crate::json::View>>,
}

/// Generate the JSON Schema for the SdaiModel type
#[cfg(feature = "schema")]
pub fn generate_schema() -> schemars::Schema {
    schemars::schema_for!(SdaiModel)
}

/// Generate the JSON Schema as a formatted JSON string
#[cfg(feature = "schema")]
pub fn generate_schema_json() -> String {
    let schema = generate_schema();
    serde_json::to_string_pretty(&schema).expect("schema serialization should never fail")
}

// Conversions FROM SDAI types TO datamodel types

impl From<GraphicalFunction> for datamodel::GraphicalFunction {
    fn from(gf: GraphicalFunction) -> Self {
        let x_points: Vec<f64> = gf.points.iter().map(|p| p.x).collect();
        let y_points: Vec<f64> = gf.points.iter().map(|p| p.y).collect();

        // Use default 0-1 scale for empty point arrays to avoid INFINITY values
        let (x_min, x_max) = if x_points.is_empty() {
            (0.0, 1.0)
        } else {
            let min = x_points.iter().copied().fold(f64::INFINITY, f64::min);
            let max = x_points.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            (min, max)
        };

        let (y_min, y_max) = if y_points.is_empty() {
            (0.0, 1.0)
        } else {
            let min = y_points.iter().copied().fold(f64::INFINITY, f64::min);
            let max = y_points.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            (min, max)
        };

        datamodel::GraphicalFunction {
            kind: datamodel::GraphicalFunctionKind::Continuous,
            x_points: Some(x_points),
            y_points,
            x_scale: datamodel::GraphicalFunctionScale {
                min: x_min,
                max: x_max,
            },
            y_scale: datamodel::GraphicalFunctionScale {
                min: y_min,
                max: y_max,
            },
        }
    }
}

impl From<StockFields> for datamodel::Stock {
    fn from(stock: StockFields) -> Self {
        let equation = datamodel::Equation::Scalar(stock.equation.unwrap_or_default());

        datamodel::Stock {
            ident: stock.name,
            equation,
            documentation: stock.documentation.unwrap_or_default(),
            units: stock.units,
            inflows: stock.inflows.unwrap_or_default(),
            outflows: stock.outflows.unwrap_or_default(),
            non_negative: false,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }
    }
}

impl From<FlowFields> for datamodel::Flow {
    fn from(flow: FlowFields) -> Self {
        let equation = datamodel::Equation::Scalar(flow.equation.unwrap_or_default());

        datamodel::Flow {
            ident: flow.name,
            equation,
            documentation: flow.documentation.unwrap_or_default(),
            units: flow.units,
            gf: flow.graphical_function.map(|gf| gf.into()),
            non_negative: false,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }
    }
}

impl From<AuxiliaryFields> for datamodel::Aux {
    fn from(aux: AuxiliaryFields) -> Self {
        let equation = datamodel::Equation::Scalar(aux.equation.unwrap_or_default());

        datamodel::Aux {
            ident: aux.name,
            equation,
            documentation: aux.documentation.unwrap_or_default(),
            units: aux.units,
            gf: aux.graphical_function.map(|gf| gf.into()),
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }
    }
}

impl From<SimSpecs> for datamodel::SimSpecs {
    fn from(specs: SimSpecs) -> Self {
        let dt = specs.dt.unwrap_or(1.0);
        let dt = if dt == 1.0 {
            datamodel::Dt::Dt(1.0)
        } else {
            datamodel::Dt::Dt(dt)
        };

        let save_step = specs.save_step.map(datamodel::Dt::Dt);

        let sim_method = match specs.method.as_deref().map(|s| s.to_lowercase()) {
            Some(ref m) if m == "rk4" => datamodel::SimMethod::RungeKutta4,
            Some(ref m) if m == "rk2" => datamodel::SimMethod::RungeKutta2,
            _ => datamodel::SimMethod::Euler,
        };

        datamodel::SimSpecs {
            start: specs.start_time,
            stop: specs.stop_time,
            dt,
            save_step,
            sim_method,
            time_units: specs.time_units,
        }
    }
}

impl From<SdaiModel> for datamodel::Project {
    fn from(sdai: SdaiModel) -> Self {
        let mut variables = Vec::new();

        for var in sdai.variables {
            match var {
                Variable::Stock(s) => {
                    variables.push(datamodel::Variable::Stock(s.into()));
                }
                Variable::Flow(f) => {
                    variables.push(datamodel::Variable::Flow(f.into()));
                }
                Variable::Variable(a) => {
                    variables.push(datamodel::Variable::Aux(a.into()));
                }
            }
        }

        let sim_specs = sdai
            .specs
            .map(|s| s.into())
            .unwrap_or_else(|| datamodel::SimSpecs {
                start: 0.0,
                stop: 100.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: None,
                sim_method: datamodel::SimMethod::Euler,
                time_units: None,
            });

        let views = sdai
            .views
            .map(|vs| vs.into_iter().map(|v| v.into()).collect())
            .unwrap_or_default();

        let model = datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables,
            views,
            loop_metadata: vec![],
            groups: vec![],
        };

        datamodel::Project {
            name: "model".to_string(),
            sim_specs,
            dimensions: vec![],
            units: vec![],
            models: vec![model],
            source: None,
            ai_information: None,
        }
    }
}

impl std::str::FromStr for SdaiModel {
    type Err = crate::common::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s).map_err(|err| {
            crate::common::Error::new(
                crate::common::ErrorKind::Import,
                crate::common::ErrorCode::Generic,
                Some(format!("Failed to parse SDAI JSON model: {}", err)),
            )
        })
    }
}

impl SdaiModel {
    /// Parse an SdaiModel from a reader
    pub fn from_reader(reader: impl std::io::Read) -> crate::common::Result<Self> {
        serde_json::from_reader(reader).map_err(|err| {
            crate::common::Error::new(
                crate::common::ErrorKind::Import,
                crate::common::ErrorCode::Generic,
                Some(format!("Failed to parse SDAI JSON model: {}", err)),
            )
        })
    }
}

// Conversions FROM datamodel types TO SDAI types

impl From<datamodel::GraphicalFunction> for GraphicalFunction {
    fn from(gf: datamodel::GraphicalFunction) -> Self {
        let points = if let Some(x_points) = gf.x_points {
            x_points
                .into_iter()
                .zip(gf.y_points)
                .map(|(x, y)| Point { x, y })
                .collect()
        } else {
            vec![]
        };

        GraphicalFunction { points }
    }
}

fn extract_equation_string(eq: &datamodel::Equation) -> String {
    match eq {
        datamodel::Equation::Scalar(s) => s.clone(),
        datamodel::Equation::ApplyToAll(_, s) => s.clone(),
        datamodel::Equation::Arrayed(_, elems) => {
            if let Some((_, s, _, _)) = elems.first() {
                s.clone()
            } else {
                String::new()
            }
        }
    }
}

/// Convert a datamodel Stock to SDAI StockFields.
///
/// Note: Empty vectors for inflows/outflows are normalized to None,
/// as are empty strings for equation, documentation, and units.
/// This means `Some([])` and `None` are semantically equivalent
/// and will both roundtrip to `None`.
impl From<datamodel::Stock> for StockFields {
    fn from(stock: datamodel::Stock) -> Self {
        let equation = extract_equation_string(&stock.equation);

        StockFields {
            name: stock.ident,
            equation: if equation.is_empty() {
                None
            } else {
                Some(equation)
            },
            documentation: if stock.documentation.is_empty() {
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
            graphical_function: None,
        }
    }
}

impl From<datamodel::Flow> for FlowFields {
    fn from(flow: datamodel::Flow) -> Self {
        let equation = extract_equation_string(&flow.equation);

        FlowFields {
            name: flow.ident,
            equation: if equation.is_empty() {
                None
            } else {
                Some(equation)
            },
            documentation: if flow.documentation.is_empty() {
                None
            } else {
                Some(flow.documentation)
            },
            units: flow.units,
            graphical_function: flow.gf.map(|gf| gf.into()),
        }
    }
}

impl From<datamodel::Aux> for AuxiliaryFields {
    fn from(aux: datamodel::Aux) -> Self {
        let equation = extract_equation_string(&aux.equation);

        AuxiliaryFields {
            name: aux.ident,
            equation: if equation.is_empty() {
                None
            } else {
                Some(equation)
            },
            documentation: if aux.documentation.is_empty() {
                None
            } else {
                Some(aux.documentation)
            },
            units: aux.units,
            graphical_function: aux.gf.map(|gf| gf.into()),
        }
    }
}

impl From<datamodel::SimSpecs> for SimSpecs {
    fn from(specs: datamodel::SimSpecs) -> Self {
        let dt = match specs.dt {
            datamodel::Dt::Dt(v) => v,
            datamodel::Dt::Reciprocal(v) => 1.0 / v,
        };

        let save_step = specs.save_step.map(|ss| match ss {
            datamodel::Dt::Dt(v) => v,
            datamodel::Dt::Reciprocal(v) => 1.0 / v,
        });

        let method = match specs.sim_method {
            datamodel::SimMethod::RungeKutta4 => Some("rk4".to_string()),
            datamodel::SimMethod::RungeKutta2 => Some("rk2".to_string()),
            datamodel::SimMethod::Euler => None,
        };

        SimSpecs {
            start_time: specs.start,
            stop_time: specs.stop,
            dt: Some(dt),
            time_units: specs.time_units,
            save_step,
            method,
        }
    }
}

impl From<datamodel::Project> for SdaiModel {
    fn from(project: datamodel::Project) -> Self {
        let model = project
            .models
            .into_iter()
            .next()
            .unwrap_or_else(|| datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            });

        let mut variables = Vec::new();

        for var in model.variables {
            match var {
                datamodel::Variable::Stock(s) => {
                    variables.push(Variable::Stock(s.into()));
                }
                datamodel::Variable::Flow(f) => {
                    variables.push(Variable::Flow(f.into()));
                }
                datamodel::Variable::Aux(a) => {
                    variables.push(Variable::Variable(a.into()));
                }
                datamodel::Variable::Module(_) => {}
            }
        }

        let specs = Some(project.sim_specs.into());

        let views = if model.views.is_empty() {
            None
        } else {
            Some(model.views.into_iter().map(|v| v.into()).collect())
        };

        SdaiModel {
            variables,
            relationships: None,
            specs,
            views,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_stock_roundtrip() {
        let sdai_stock = StockFields {
            name: "inventory".to_string(),
            equation: Some("100".to_string()),
            documentation: Some("Current inventory".to_string()),
            units: Some("widgets".to_string()),
            inflows: Some(vec!["production".to_string()]),
            outflows: Some(vec!["sales".to_string()]),
            graphical_function: None,
        };

        let dm_stock: datamodel::Stock = sdai_stock.clone().into();
        let sdai_stock2: StockFields = dm_stock.into();

        assert_eq!(sdai_stock.name, sdai_stock2.name);
        assert_eq!(sdai_stock.equation, sdai_stock2.equation);
        assert_eq!(sdai_stock.inflows, sdai_stock2.inflows);
        assert_eq!(sdai_stock.outflows, sdai_stock2.outflows);
    }

    #[test]
    fn test_basic_flow_roundtrip() {
        let sdai_flow = FlowFields {
            name: "production".to_string(),
            equation: Some("10".to_string()),
            documentation: None,
            units: Some("widgets/month".to_string()),
            graphical_function: None,
        };

        let dm_flow: datamodel::Flow = sdai_flow.clone().into();
        let sdai_flow2: FlowFields = dm_flow.into();

        assert_eq!(sdai_flow.name, sdai_flow2.name);
        assert_eq!(sdai_flow.equation, sdai_flow2.equation);
        assert_eq!(sdai_flow.units, sdai_flow2.units);
    }

    #[test]
    fn test_basic_auxiliary_roundtrip() {
        let sdai_aux = AuxiliaryFields {
            name: "target".to_string(),
            equation: Some("200".to_string()),
            documentation: Some("Target level".to_string()),
            units: None,
            graphical_function: None,
        };

        let dm_aux: datamodel::Aux = sdai_aux.clone().into();
        let sdai_aux2: AuxiliaryFields = dm_aux.into();

        assert_eq!(sdai_aux.name, sdai_aux2.name);
        assert_eq!(sdai_aux.equation, sdai_aux2.equation);
        assert_eq!(sdai_aux.documentation, sdai_aux2.documentation);
    }

    #[test]
    fn test_graphical_function_roundtrip() {
        let sdai_gf = GraphicalFunction {
            points: vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: 1.0, y: 10.0 },
                Point { x: 2.0, y: 15.0 },
            ],
        };

        let dm_gf: datamodel::GraphicalFunction = sdai_gf.clone().into();

        assert_eq!(dm_gf.kind, datamodel::GraphicalFunctionKind::Continuous);
        assert_eq!(dm_gf.x_points.as_ref().unwrap().len(), 3);
        assert_eq!(dm_gf.y_points.len(), 3);

        let sdai_gf2: GraphicalFunction = dm_gf.into();

        assert_eq!(sdai_gf.points.len(), sdai_gf2.points.len());
        for (p1, p2) in sdai_gf.points.iter().zip(sdai_gf2.points.iter()) {
            assert!((p1.x - p2.x).abs() < 1e-10);
            assert!((p1.y - p2.y).abs() < 1e-10);
        }
    }

    #[test]
    fn test_sim_specs_roundtrip() {
        let sdai_specs = SimSpecs {
            start_time: 0.0,
            stop_time: 100.0,
            dt: Some(0.25),
            time_units: Some("months".to_string()),
            save_step: Some(1.0),
            method: Some("rk4".to_string()),
        };

        let dm_specs: datamodel::SimSpecs = sdai_specs.clone().into();
        let sdai_specs2: SimSpecs = dm_specs.into();

        assert_eq!(sdai_specs.start_time, sdai_specs2.start_time);
        assert_eq!(sdai_specs.stop_time, sdai_specs2.stop_time);
        assert_eq!(sdai_specs.dt, sdai_specs2.dt);
        assert_eq!(sdai_specs.time_units, sdai_specs2.time_units);
        assert_eq!(sdai_specs.method, sdai_specs2.method);
    }

    #[test]
    fn test_full_model_roundtrip() {
        let sdai_model = SdaiModel {
            variables: vec![
                Variable::Stock(StockFields {
                    name: "inventory".to_string(),
                    equation: Some("50".to_string()),
                    documentation: None,
                    units: Some("widgets".to_string()),
                    inflows: Some(vec!["production".to_string()]),
                    outflows: Some(vec!["sales".to_string()]),
                    graphical_function: None,
                }),
                Variable::Flow(FlowFields {
                    name: "production".to_string(),
                    equation: Some("10".to_string()),
                    documentation: None,
                    units: Some("widgets/month".to_string()),
                    graphical_function: None,
                }),
                Variable::Flow(FlowFields {
                    name: "sales".to_string(),
                    equation: Some("8".to_string()),
                    documentation: None,
                    units: Some("widgets/month".to_string()),
                    graphical_function: None,
                }),
                Variable::Variable(AuxiliaryFields {
                    name: "target_inventory".to_string(),
                    equation: Some("100".to_string()),
                    documentation: None,
                    units: Some("widgets".to_string()),
                    graphical_function: None,
                }),
            ],
            relationships: None,
            specs: Some(SimSpecs {
                start_time: 0.0,
                stop_time: 10.0,
                dt: Some(1.0),
                time_units: Some("months".to_string()),
                save_step: None,
                method: None,
            }),
            views: None,
        };

        let dm_project: datamodel::Project = sdai_model.clone().into();

        assert_eq!(dm_project.models.len(), 1);
        assert_eq!(dm_project.models[0].variables.len(), 4);

        let sdai_model2: SdaiModel = dm_project.into();

        assert_eq!(sdai_model.variables.len(), sdai_model2.variables.len());
    }

    #[test]
    fn test_json_serialization() {
        let sdai_model = SdaiModel {
            variables: vec![
                Variable::Stock(StockFields {
                    name: "inventory".to_string(),
                    equation: Some("100".to_string()),
                    documentation: None,
                    units: Some("widgets".to_string()),
                    inflows: Some(vec!["inflow".to_string()]),
                    outflows: Some(vec!["outflow".to_string()]),
                    graphical_function: None,
                }),
                Variable::Flow(FlowFields {
                    name: "inflow".to_string(),
                    equation: Some("5".to_string()),
                    documentation: None,
                    units: None,
                    graphical_function: None,
                }),
            ],
            relationships: None,
            specs: Some(SimSpecs {
                start_time: 0.0,
                stop_time: 100.0,
                dt: Some(1.0),
                time_units: None,
                save_step: None,
                method: None,
            }),
            views: None,
        };

        let json_str = serde_json::to_string_pretty(&sdai_model).unwrap();

        assert!(json_str.contains("\"type\": \"stock\""));
        assert!(json_str.contains("\"type\": \"flow\""));
        assert!(json_str.contains("\"inventory\""));
        assert!(json_str.contains("\"startTime\""));
        assert!(json_str.contains("\"stopTime\""));

        let sdai_model2: SdaiModel = serde_json::from_str(&json_str).unwrap();

        assert_eq!(sdai_model.variables.len(), sdai_model2.variables.len());
    }

    #[test]
    fn test_optional_fields() {
        let json_str = r#"{
            "variables": [
                {
                    "type": "stock",
                    "name": "inventory"
                }
            ]
        }"#;

        let sdai_model: SdaiModel = serde_json::from_str(json_str).unwrap();

        assert_eq!(sdai_model.variables.len(), 1);

        if let Variable::Stock(stock) = &sdai_model.variables[0] {
            assert_eq!(stock.name, "inventory");
            assert_eq!(stock.equation, None);
            assert_eq!(stock.inflows, None);
        } else {
            panic!("Expected stock variable");
        }
    }

    #[test]
    fn test_with_graphical_function() {
        let json_str = r#"{
            "variables": [
                {
                    "type": "flow",
                    "name": "rate",
                    "equation": "5",
                    "graphicalFunction": {
                        "points": [
                            {"x": 0.0, "y": 0.0},
                            {"x": 5.0, "y": 10.0},
                            {"x": 10.0, "y": 5.0}
                        ]
                    }
                }
            ],
            "specs": {
                "startTime": 0,
                "stopTime": 10
            }
        }"#;

        let sdai_model: SdaiModel = serde_json::from_str(json_str).unwrap();

        if let Variable::Flow(flow) = &sdai_model.variables[0] {
            assert!(flow.graphical_function.is_some());
            let gf = flow.graphical_function.as_ref().unwrap();
            assert_eq!(gf.points.len(), 3);
        } else {
            panic!("Expected flow variable");
        }

        let dm_project: datamodel::Project = sdai_model.into();
        let model = &dm_project.models[0];

        if let datamodel::Variable::Flow(flow) = &model.variables[0] {
            assert!(flow.gf.is_some());
        } else {
            panic!("Expected flow in datamodel");
        }
    }

    #[test]
    fn test_relationships_ignored() {
        let json_str = r#"{
            "variables": [
                {
                    "type": "variable",
                    "name": "x",
                    "equation": "5"
                }
            ],
            "relationships": [
                {
                    "from": "a",
                    "to": "b",
                    "polarity": "+"
                }
            ]
        }"#;

        let sdai_model: SdaiModel = serde_json::from_str(json_str).unwrap();

        assert!(sdai_model.relationships.is_some());
        assert_eq!(sdai_model.relationships.as_ref().unwrap().len(), 1);

        let _dm_project: datamodel::Project = sdai_model.into();
    }
}
