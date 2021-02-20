// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::io::{BufRead, Cursor, Write};

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Writer;
use serde::{Deserialize, Serialize};

use crate::xmile::view_element::LinkEnd;
use system_dynamics_engine::common::{canonicalize, Result};
use system_dynamics_engine::datamodel;
use system_dynamics_engine::datamodel::{Equation, Rect, ViewElement};

trait ToXML<W: Clone + Write> {
    fn write_xml(&self, writer: &mut Writer<W>) -> Result<()>;
}

type XMLWriter = Cursor<Vec<u8>>;

const STOCK_WIDTH: f64 = 45.0;
const STOCK_HEIGHT: f64 = 35.0;

macro_rules! import_err(
    ($code:tt, $str:expr) => {{
        use system_dynamics_engine::common::{Error, ErrorCode, ErrorKind};
        Err(Error::new(ErrorKind::Model, ErrorCode::$code, Some($str)))
    }}
);

const XMILE_VERSION: &str = "1.0";
// const NS_HTTPS: &str = "https://docs.oasis-open.org/xmile/ns/XMILE/v1.0";
const NS_HTTP: &str = "http://docs.oasis-open.org/xmile/ns/XMILE/v1.0";
const NS_ISEE_HTTP: &str = "http://iseesystems.com/XMILE";
const VENDOR: &str = "Simlin";
const PRODUCT_VERSION: &str = "0.1.0";
const PRODUCT_NAME: &str = "Simlin";
const PRODUCT_LANG: &str = "en";

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

impl ToXML<XMLWriter> for File {
    fn write_xml(&self, writer: &mut Writer<XMLWriter>) -> Result<()> {
        // xmile tag
        let attrs = &[
            ("version", self.version.as_str()),
            ("xmlns", self.namespace.as_str()),
            ("xmlns:isee", NS_ISEE_HTTP),
        ];
        write_tag_start_with_attrs(writer, "xmile", attrs)?;

        if let Some(ref header) = self.header {
            header.write_xml(writer)?;
        }

        if let Some(ref sim_specs) = self.sim_specs {
            sim_specs.write_xml(writer)?;
        }

        // TODO

        write_tag_end(writer, "xmile")
    }
}

impl From<File> for datamodel::Project {
    fn from(file: File) -> Self {
        datamodel::Project {
            name: file
                .header
                .as_ref()
                .map(|header| header.name.clone().unwrap_or_default())
                .unwrap_or_default(),
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

impl From<datamodel::Project> for File {
    fn from(project: datamodel::Project) -> Self {
        File {
            version: XMILE_VERSION.to_owned(),
            namespace: NS_HTTP.to_owned(),
            header: Some(Header {
                vendor: VENDOR.to_owned(),
                product: Product {
                    name: Some(PRODUCT_NAME.to_owned()),
                    language: Some(PRODUCT_LANG.to_owned()),
                    version: Some(PRODUCT_VERSION.to_owned()),
                },
                options: None,
                name: if project.name.is_empty() {
                    None
                } else {
                    Some(project.name)
                },
                version: None,
                caption: None,
                image: None,
                author: None,
                affiliation: None,
                client: None,
                copyright: None,
                created: None,
                modified: None,
                uuid: None,
                includes: None,
            }),
            sim_specs: Some(project.sim_specs.into()),
            units: None,
            dimensions: if project.dimensions.is_empty() {
                None
            } else {
                Some(Dimensions {
                    dimensions: Some(
                        project
                            .dimensions
                            .into_iter()
                            .map(Dimension::from)
                            .collect(),
                    ),
                })
            },
            behavior: None,
            style: None,
            data: None,
            models: project.models.into_iter().map(Model::from).collect(),
            macros: vec![],
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

fn xml_error(err: quick_xml::Error) -> system_dynamics_engine::common::Error {
    use system_dynamics_engine::common::{Error, ErrorCode, ErrorKind};

    Error::new(
        ErrorKind::Import,
        ErrorCode::XmlDeserialization,
        Some(err.to_string()),
    )
}

fn write_tag_start(writer: &mut Writer<XMLWriter>, tag_name: &str) -> Result<()> {
    write_tag_start_with_attrs(writer, tag_name, &[])
}

fn write_tag_start_with_attrs(
    writer: &mut Writer<XMLWriter>,
    tag_name: &str,
    attrs: &[(&str, &str)],
) -> Result<()> {
    let mut elem = BytesStart::owned(tag_name.as_bytes().to_vec(), tag_name.len());
    for attr in attrs.iter() {
        elem.push_attribute(*attr);
    }
    writer.write_event(Event::Start(elem)).map_err(xml_error)
}

fn write_tag_end(writer: &mut Writer<XMLWriter>, tag_name: &str) -> Result<()> {
    writer
        .write_event(Event::End(BytesEnd::borrowed(tag_name.as_bytes())))
        .map_err(xml_error)
}

fn write_tag_text(writer: &mut Writer<XMLWriter>, content: &str) -> Result<()> {
    writer
        .write_event(Event::Text(BytesText::from_plain_str(content)))
        .map_err(xml_error)
}

fn write_tag(writer: &mut Writer<XMLWriter>, tag_name: &str, content: &str) -> Result<()> {
    write_tag_with_attrs(writer, tag_name, content, &[])
}

fn write_tag_with_attrs(
    writer: &mut Writer<XMLWriter>,
    tag_name: &str,
    content: &str,
    attrs: &[(&str, &str)],
) -> Result<()> {
    write_tag_start_with_attrs(writer, tag_name, attrs)?;

    write_tag_text(writer, content)?;

    write_tag_end(writer, tag_name)
}

impl ToXML<XMLWriter> for Header {
    fn write_xml(&self, writer: &mut Writer<XMLWriter>) -> Result<()> {
        // header tag
        write_tag_start(writer, "header")?;

        // name tag
        if let Some(ref name) = self.name {
            write_tag(writer, "name", name)?;
        }

        // vendor
        write_tag(writer, "vendor", self.vendor.as_str())?;

        // product
        {
            let mut attrs = Vec::with_capacity(2);
            if let Some(ref version) = self.product.version {
                attrs.push(("version", version.as_str()));
            }
            if let Some(ref language) = self.product.language {
                attrs.push(("lang", language.as_str()));
            }
            let name: &str = self.product.name.as_deref().unwrap_or("Simlin");
            write_tag_with_attrs(writer, "product", name, &attrs)?;
        }

        write_tag_end(writer, "header")
    }
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
    #[serde(rename = "isee:save_interval")]
    pub save_step: Option<f64>,
    pub method: Option<String>,
    pub time_units: Option<String>,
}

impl ToXML<XMLWriter> for SimSpecs {
    fn write_xml(&self, writer: &mut Writer<XMLWriter>) -> Result<()> {
        let mut elem = BytesStart::owned(b"sim_specs".to_vec(), b"sim_specs".len());
        if let Some(ref method) = self.method {
            elem.push_attribute(("method", method.as_str()));
        }
        if let Some(ref time_units) = self.time_units {
            elem.push_attribute(("time_units", time_units.as_str()));
        }
        if let Some(ref save_step) = self.save_step {
            let save_interval = format!("{}", save_step);
            elem.push_attribute(("isee:save_interval", save_interval.as_str()));
        }
        writer.write_event(Event::Start(elem)).map_err(xml_error)?;

        let start = format!("{}", self.start);
        write_tag(writer, "start", &start)?;

        let stop = format!("{}", self.stop);
        write_tag(writer, "stop", &stop)?;

        if let Some(ref dt) = self.dt {
            let value = format!("{}", dt.value);
            if dt.reciprocal.unwrap_or(false) {
                let attrs = &[("reciprocal", "true")];
                write_tag_with_attrs(writer, "dt", &value, attrs)?;
            } else {
                write_tag(writer, "dt", &value)?;
            }
        }

        write_tag_end(writer, "sim_specs")
    }
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
            name: canonicalize(&dimension.name),
            elements: dimension
                .elements
                .unwrap_or_default()
                .into_iter()
                .map(|i| canonicalize(&i.name))
                .collect(),
        }
    }
}

impl From<datamodel::Dimension> for Dimension {
    fn from(dimension: datamodel::Dimension) -> Self {
        Dimension {
            name: dimension.name,
            size: None,
            elements: Some(
                dimension
                    .elements
                    .into_iter()
                    .map(|i| Index { name: i })
                    .collect(),
            ),
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
            .filter(|v| v.kind.unwrap_or(ViewType::StockFlow) == ViewType::StockFlow)
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
                    .filter(|v| !matches!(v, Var::Unhandled))
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
    use crate::xmile::{STOCK_HEIGHT, STOCK_WIDTH};
    use serde::{de, Deserialize, Deserializer, Serialize};
    use system_dynamics_engine::datamodel::view_element::LinkShape;

    // converts an angle associated with a connector (in degrees) into an
    // angle in the coordinate system of SVG canvases where the origin is
    // in the upper-left of the screen and Y grows down, and the domain is
    // -180 to 180.
    fn convert_angle_from_xmile_to_canvas(in_degrees: f64) -> f64 {
        let out_degrees = (360.0 - in_degrees) % 360.0;
        if out_degrees > 180.0 {
            out_degrees - 360.0
        } else {
            out_degrees
        }
    }

    // converts an angle associated with a connector (in degrees) into an
    // angle in the coordinate system of SVG canvases where the origin is
    // in the upper-left of the screen and Y grows down, and the domain is
    // -180 to 180.
    fn convert_angle_from_canvas_to_xmile(in_degrees: f64) -> f64 {
        let out_degrees = if in_degrees < 0.0 {
            in_degrees + 360.0
        } else {
            in_degrees
        };
        (360.0 - out_degrees) % 360.0
    }

    #[test]
    fn test_convert_angles() {
        let cases: &[(f64, f64)] = &[(0.0, 0.0), (45.0, -45.0), (270.0, 90.0)];

        for (input, output) in cases {
            assert_eq!(*output, convert_angle_from_xmile_to_canvas(*input));
            assert_eq!(*input, convert_angle_from_canvas_to_xmile(*output));
        }
    }

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

    impl Stock {
        pub fn is_right(&self, pt: &Point) -> bool {
            pt.x > self.x + STOCK_WIDTH / 2.0 && (pt.y - self.y).abs() < STOCK_HEIGHT / 2.0
        }
        pub fn is_left(&self, pt: &Point) -> bool {
            pt.x < self.x + STOCK_WIDTH / 2.0 && (pt.y - self.y).abs() < STOCK_HEIGHT / 2.0
        }
        pub fn is_above(&self, pt: &Point) -> bool {
            pt.y < self.y + STOCK_HEIGHT / 2.0 && (pt.x - self.x).abs() < STOCK_WIDTH / 2.0
        }
        pub fn is_below(&self, pt: &Point) -> bool {
            pt.y > self.y + STOCK_HEIGHT / 2.0 && (pt.x - self.x).abs() < STOCK_WIDTH / 2.0
        }
    }

    impl From<Stock> for datamodel::view_element::Stock {
        fn from(v: Stock) -> Self {
            let x = match v.width {
                Some(w) => v.x + w / 2.0,
                None => v.x,
            };
            let y = match v.height {
                Some(h) => v.y + h / 2.0,
                None => v.y,
            };
            datamodel::view_element::Stock {
                name: v.name,
                uid: v.uid.unwrap_or(-1),
                x,
                y,
                // isee's default label side is top
                label_side: datamodel::view_element::LabelSide::from(
                    v.label_side.unwrap_or(LabelSide::Top),
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

    fn is_horizontal(points: &[datamodel::view_element::FlowPoint]) -> bool {
        if points.len() > 2 {
            return false;
        }
        let start = &points[0];
        let end = &points[1];
        let dx = (end.x - start.x).abs();
        let dy = (end.y - start.y).abs();

        dx > dy
    }

    impl From<Flow> for datamodel::view_element::Flow {
        fn from(v: Flow) -> Self {
            // position of the flow valve
            let mut cx = v.x;
            let mut cy = v.y;
            let mut points: Vec<_> = v
                .points
                .unwrap_or_default()
                .points
                .into_iter()
                .map(datamodel::view_element::FlowPoint::from)
                .collect();
            // Vensim imports don't actually enforce horizontal or vertical lines are straight
            if points.len() == 2 {
                if is_horizontal(&points) {
                    let new_y = (points[0].y + points[1].y) / 2.0;
                    points[0].y = new_y;
                    points[1].y = new_y;
                    cy = new_y;
                } else {
                    let new_x = (points[0].x + points[1].x) / 2.0;
                    points[0].x = new_x;
                    points[1].x = new_x;
                    cx = new_x;
                }
            }
            datamodel::view_element::Flow {
                name: v.name,
                uid: v.uid.unwrap_or(-1),
                x: cx,
                y: cy,
                label_side: datamodel::view_element::LabelSide::from(
                    v.label_side.unwrap_or(LabelSide::Bottom),
                ),
                points,
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

        let input_v = datamodel::view_element::Flow {
            name: "from_vensim_v".to_string(),
            uid: 76,
            x: 2.0,
            y: 5.0,
            label_side: datamodel::view_element::LabelSide::Bottom,
            points: vec![
                datamodel::view_element::FlowPoint {
                    x: 1.0,
                    y: 1.0,
                    attached_to_uid: None,
                },
                datamodel::view_element::FlowPoint {
                    x: 3.0,
                    y: 9.0,
                    attached_to_uid: None,
                },
            ],
        };
        let expected_v = datamodel::view_element::Flow {
            name: "from_vensim_v".to_string(),
            uid: 76,
            x: 2.0,
            y: 5.0,
            label_side: datamodel::view_element::LabelSide::Bottom,
            points: vec![
                datamodel::view_element::FlowPoint {
                    x: 2.0,
                    y: 1.0,
                    attached_to_uid: None,
                },
                datamodel::view_element::FlowPoint {
                    x: 2.0,
                    y: 9.0,
                    attached_to_uid: None,
                },
            ],
        };
        let actual_v = datamodel::view_element::Flow::from(Flow::from(input_v.clone()));
        assert_eq!(expected_v, actual_v);

        let input_h = datamodel::view_element::Flow {
            name: "from_vensim_h".to_string(),
            uid: 76,
            x: 5.0,
            y: 2.0,
            label_side: datamodel::view_element::LabelSide::Bottom,
            points: vec![
                datamodel::view_element::FlowPoint {
                    x: 1.0,
                    y: 1.0,
                    attached_to_uid: None,
                },
                datamodel::view_element::FlowPoint {
                    x: 9.0,
                    y: 3.0,
                    attached_to_uid: None,
                },
            ],
        };
        let expected_h = datamodel::view_element::Flow {
            name: "from_vensim_h".to_string(),
            uid: 76,
            x: 5.0,
            y: 2.0,
            label_side: datamodel::view_element::LabelSide::Bottom,
            points: vec![
                datamodel::view_element::FlowPoint {
                    x: 1.0,
                    y: 2.0,
                    attached_to_uid: None,
                },
                datamodel::view_element::FlowPoint {
                    x: 9.0,
                    y: 2.0,
                    attached_to_uid: None,
                },
            ],
        };
        let actual_h = datamodel::view_element::Flow::from(Flow::from(input_h.clone()));
        assert_eq!(expected_h, actual_h);
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct AliasLinkEnd {
        pub uid: i32,
    }

    #[derive(Clone, PartialEq, Debug, Serialize)]
    pub enum LinkEnd {
        #[serde(rename = "$value")]
        Named(String),
        #[serde(rename = "alias")]
        Alias(AliasLinkEnd),
    }

    // this is hacked up from the derived Deserialize method, but now works with the
    // bad way 'from' tags are done for Connectors that start on an alias.
    impl<'de> Deserialize<'de> for LinkEnd {
        fn deserialize<V>(__deserializer: V) -> std::result::Result<Self, V::Error>
        where
            V: Deserializer<'de>,
        {
            enum Field {
                Field0,
                Field1,
            }
            struct __FieldVisitor;
            impl<'de> de::Visitor<'de> for __FieldVisitor {
                type Value = Field;
                fn expecting(&self, __formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                    __formatter.write_str("variant identifier")
                }

                fn visit_u64<U>(self, __value: u64) -> std::result::Result<Self::Value, U>
                where
                    U: de::Error,
                {
                    match __value {
                        0u64 => Ok(Field::Field0),
                        1u64 => Ok(Field::Field1),
                        _ => Err(de::Error::invalid_value(
                            de::Unexpected::Unsigned(__value),
                            &"variant index 0 <= i < 2",
                        )),
                    }
                }
                fn visit_str<U>(self, __value: &str) -> std::result::Result<Self::Value, U>
                where
                    U: de::Error,
                {
                    match __value {
                        "alias" => Ok(Field::Field1),
                        _ => Ok(Field::Field0),
                    }
                }

                fn visit_bytes<U>(self, __value: &[u8]) -> std::result::Result<Self::Value, U>
                where
                    U: de::Error,
                {
                    match __value {
                        b"$value" => Ok(Field::Field0),
                        b"alias" => Ok(Field::Field1),
                        _ => {
                            let __value = &std::string::String::from_utf8_lossy(__value);
                            Err(de::Error::unknown_variant(__value, VARIANTS))
                        }
                    }
                }
            }

            impl<'de> Deserialize<'de> for Field {
                #[inline]
                fn deserialize<V>(__deserializer: V) -> std::result::Result<Self, V::Error>
                where
                    V: Deserializer<'de>,
                {
                    Deserializer::deserialize_identifier(__deserializer, __FieldVisitor)
                }
            }
            struct __Visitor<'de> {
                marker: std::marker::PhantomData<LinkEnd>,
                lifetime: std::marker::PhantomData<&'de ()>,
            }
            impl<'de> de::Visitor<'de> for __Visitor<'de> {
                type Value = LinkEnd;
                fn expecting(&self, __formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                    __formatter.write_str("enum LinkEnd")
                }
                fn visit_enum<__A>(
                    self,
                    __data: __A,
                ) -> std::result::Result<Self::Value, __A::Error>
                where
                    __A: de::EnumAccess<'de>,
                {
                    match match de::EnumAccess::variant(__data) {
                        Ok(__val) => __val,
                        Err(__err) => {
                            return Err(__err);
                        }
                    } {
                        (Field::Field0, __variant) => std::result::Result::map(
                            de::VariantAccess::newtype_variant::<String>(__variant),
                            LinkEnd::Named,
                        ),
                        (Field::Field1, __variant) => std::result::Result::map(
                            de::VariantAccess::newtype_variant::<AliasLinkEnd>(__variant),
                            LinkEnd::Alias,
                        ),
                    }
                }
            }
            const VARIANTS: &[&str] = &["$value", "alias"];
            __deserializer.deserialize_enum(
                "LinkEnd",
                VARIANTS,
                __Visitor {
                    marker: std::marker::PhantomData::<LinkEnd>,
                    lifetime: std::marker::PhantomData,
                },
            )
        }
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct LinkEndContainer {
        #[serde(rename = "$value")]
        pub end: LinkEnd,
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Link {
        pub uid: Option<i32>,
        pub from: LinkEndContainer,
        pub from_uid: Option<i32>,
        #[serde(rename = "to")]
        pub to: LinkEndContainer,
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
                datamodel::view_element::LinkShape::Arc(convert_angle_from_canvas_to_xmile(
                    v.angle.unwrap_or(0.0),
                ))
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
                LinkShape::Arc(angle) => {
                    (None, Some(convert_angle_from_xmile_to_canvas(angle)), None)
                }
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
                from: LinkEndContainer {
                    end: LinkEnd::Named("".to_owned()),
                },
                from_uid: Some(v.from_uid),
                to: LinkEndContainer {
                    end: LinkEnd::Named("".to_owned()),
                },
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
    pub struct Alias {
        pub of: String,
        pub of_uid: Option<i32>,
        pub uid: Option<i32>,
        pub x: f64,
        pub y: f64,
        pub label_side: Option<LabelSide>,
    }

    impl From<Alias> for datamodel::view_element::Alias {
        fn from(v: Alias) -> Self {
            datamodel::view_element::Alias {
                uid: v.uid.unwrap_or(-1),
                alias_of_uid: v.of_uid.unwrap_or(-1),
                x: v.x,
                y: v.y,
                label_side: datamodel::view_element::LabelSide::from(
                    v.label_side.unwrap_or(LabelSide::Bottom),
                ),
            }
        }
    }

    impl From<datamodel::view_element::Alias> for Alias {
        fn from(v: datamodel::view_element::Alias) -> Self {
            Alias {
                uid: Some(v.uid),
                of: "".to_owned(),
                of_uid: Some(v.alias_of_uid),
                x: v.x,
                y: v.y,
                label_side: Some(LabelSide::from(v.label_side)),
            }
        }
    }

    #[test]
    fn test_alias_roundtrip() {
        let cases: &[_] = &[datamodel::view_element::Alias {
            uid: 33,
            alias_of_uid: 2,
            x: 74.0,
            y: 31.0,
            label_side: datamodel::view_element::LabelSide::Right,
        }];
        for expected in cases {
            let expected = expected.clone();
            let actual = datamodel::view_element::Alias::from(Alias::from(expected.clone()));
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
    Alias(view_element::Alias),
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
            ViewObject::Alias(alias) => alias.uid = Some(uid),
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
            ViewObject::Alias(alias) => alias.uid,
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
            ViewObject::Alias(_alias) => None,
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
            ViewObject::Alias(v) => {
                datamodel::ViewElement::Alias(datamodel::view_element::Alias::from(v))
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
            ViewElement::Alias(v) => ViewObject::Alias(view_element::Alias::from(v)),
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
    pub zoom: Option<f64>,
    pub offset_x: Option<f64>,
    pub offset_y: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
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
        let mut orig_uid_map: HashMap<i32, i32> = HashMap::new();
        let mut next_uid = 1;
        for o in self.objects.iter_mut() {
            if let Some(orig_uid) = o.uid() {
                orig_uid_map.insert(orig_uid, next_uid);
            }
            o.set_uid(next_uid);
            if let Some(ident) = o.ident() {
                uid_map.insert(ident, next_uid);
            }
            next_uid += 1;
        }
        for o in self.objects.iter_mut() {
            if let ViewObject::Link(link) = o {
                link.from_uid = match &link.from.end {
                    LinkEnd::Named(name) => uid_map.get(&canonicalize(name)).cloned(),
                    LinkEnd::Alias(orig_alias) => orig_uid_map.get(&orig_alias.uid).cloned(),
                };
                link.to_uid = match &link.to.end {
                    LinkEnd::Named(name) => uid_map.get(&canonicalize(name)).cloned(),
                    LinkEnd::Alias(orig_alias) => orig_uid_map.get(&orig_alias.uid).cloned(),
                };
            } else if let ViewObject::Alias(alias) = o {
                let of_ident = canonicalize(&alias.of);
                alias.of_uid = if !of_ident.is_empty() {
                    uid_map.get(&of_ident).cloned()
                } else {
                    None
                };
            }
        }

        // if there were links we couldn't resolve, dump them
        self.objects = self
            .objects
            .iter()
            .cloned()
            .filter(|o| {
                if let ViewObject::Link(link) = o {
                    link.from_uid.is_some() && link.to_uid.is_some()
                } else {
                    true
                }
            })
            .collect();

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

    fn fixup_flow_takeoffs(&mut self) {
        let stocks: HashMap<_, _> = self
            .objects
            .iter()
            .filter(|vo| matches!(vo, ViewObject::Stock(_)))
            .cloned()
            .map(|vo| (vo.uid().unwrap(), vo))
            .collect();
        let maybe_fixup_takeoff = |pt1: &mut view_element::Point, pt2: &view_element::Point| {
            if let Some(source_uid) = pt1.uid {
                if let Some(ViewObject::Stock(stock)) = stocks.get(&source_uid) {
                    if stock.is_right(pt2) {
                        pt1.x = stock.x + STOCK_WIDTH / 2.0;
                    } else if stock.is_left(pt2) {
                        pt1.x = stock.x - STOCK_WIDTH / 2.0;
                    } else if stock.is_above(pt2) {
                        pt1.y = stock.y - STOCK_HEIGHT / 2.0;
                    } else if stock.is_below(pt2) {
                        pt1.y = stock.y + STOCK_HEIGHT / 2.0;
                    }
                }
            }
        };

        for view_object in self.objects.iter_mut() {
            if let ViewObject::Flow(flow) = view_object {
                if flow.points.is_none() || flow.points.as_ref().unwrap().points.len() != 2 {
                    continue;
                }
                let source_point = flow
                    .points
                    .as_ref()
                    .unwrap()
                    .points
                    .first()
                    .unwrap()
                    .clone();
                let sink_point = flow.points.as_ref().unwrap().points.last().unwrap().clone();
                maybe_fixup_takeoff(
                    flow.points.as_mut().unwrap().points.first_mut().unwrap(),
                    &sink_point,
                );
                maybe_fixup_takeoff(
                    flow.points.as_mut().unwrap().points.last_mut().unwrap(),
                    &source_point,
                );
            }
        }
    }

    fn normalize(&mut self, model: &Model) {
        if self.kind.unwrap_or(ViewType::StockFlow) != ViewType::StockFlow {
            return;
        }
        let uid_map = self.assign_uids();
        self.fixup_clouds(model, &uid_map);
        self.fixup_flow_takeoffs();
    }
}

impl From<View> for datamodel::View {
    fn from(v: View) -> Self {
        if v.kind.unwrap_or(ViewType::StockFlow) == ViewType::StockFlow {
            let view_box = if v.offset_x.is_some()
                && v.offset_y.is_some()
                && v.width.is_some()
                && v.height.is_some()
            {
                Rect {
                    x: v.offset_x.unwrap(),
                    y: v.offset_y.unwrap(),
                    width: v.width.unwrap(),
                    height: v.height.unwrap(),
                }
            } else {
                Default::default()
            };

            datamodel::View::StockFlow(datamodel::StockFlow {
                elements: v
                    .objects
                    .into_iter()
                    .filter(|v| !matches!(v, ViewObject::Unhandled))
                    .map(datamodel::ViewElement::from)
                    .collect(),
                view_box,
                zoom: v.zoom.unwrap_or(1.0),
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
                zoom: Some(v.zoom),
                offset_x: Some(v.view_box.x),
                offset_y: Some(v.view_box.y),
                width: Some(v.view_box.width),
                height: Some(v.view_box.height),
            },
        }
    }
}

#[test]
fn test_view_roundtrip() {
    use system_dynamics_engine::datamodel::Rect;
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
        view_box: Rect {
            x: 2.4,
            y: 9.5,
            width: 102.3,
            height: 555.3,
        },
        zoom: 1.6,
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
                Some(dimensions) => dimensions.dimensions.unwrap().into_iter().map(|e| canonicalize(&e.name)).collect(),
                None => vec![],
            };
            let elements = elements.into_iter().map(|e| {
                let canonical_subscripts: Vec<_> = e.subscript.split(",").map(|s| canonicalize(s.trim())).collect();
                (canonical_subscripts.join(","), e.eqn)
            }).collect();
            datamodel::Equation::Arrayed(dimensions, elements)
        } else if let Some(dimensions) = $var.dimensions {
            let dimensions = dimensions.dimensions.unwrap_or_default().into_iter().map(|e| canonicalize(&e.name)).collect();
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
    // for things we don't care about like 'isee:dependencies'
    #[serde(other)]
    Unhandled,
}

impl Var {
    pub fn get_noncanonical_name(&self) -> &str {
        match self {
            Var::Stock(stock) => stock.name.as_str(),
            Var::Flow(flow) => flow.name.as_str(),
            Var::Aux(aux) => aux.name.as_str(),
            Var::Module(module) => module.name.as_str(),
            Var::Unhandled => unreachable!(),
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
            Var::Unhandled => unreachable!(),
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

pub fn string_from_project(project: &datamodel::Project) -> Result<String> {
    let file: File = project.clone().into();

    let mut writer = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 4);

    writer
        .write_event(Event::Decl(BytesDecl::new(
            "1.0".as_bytes(),
            Some("utf-8".as_bytes()),
            None,
        )))
        .unwrap();
    file.write_xml(&mut writer)?;

    let result = writer.into_inner().into_inner();

    use system_dynamics_engine::common::{Error, ErrorCode, ErrorKind};
    String::from_utf8(result).map_err(|_err| {
        Error::new(
            ErrorKind::Import,
            ErrorCode::XmlDeserialization,
            Some("problem converting to UTF-8".to_owned()),
        )
    })
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
fn test_xml_gt_parsing() {
    let input = "<aux name=\"test_gt\">
                <eqn>( IF Time &gt; 25 THEN 5 ELSE 0 )</eqn>
            </aux>";
    let expected = Aux {
        name: "test_gt".to_string(),
        eqn: Some("( IF Time > 25 THEN 5 ELSE 0 )".to_string()),
        doc: None,
        units: None,
        gf: None,
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
    let input = "<sim_specs method=\"euler\" time_units=\"Time\" isee:save_interval=\"1\">
		<start>0</start>
		<stop>100</stop>
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
