// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::io::{BufRead, Cursor, Write};

use crate::xmile::view_element::LinkEnd;
use float_cmp::approx_eq;
use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use simlin_core::common::{Result, canonicalize};
use simlin_core::datamodel;
use simlin_core::datamodel::Visibility;
use simlin_core::datamodel::{Equation, Rect, ViewElement};

trait ToXml<W: Clone + Write> {
    fn write_xml(&self, writer: &mut Writer<W>) -> Result<()>;
}

type XmlWriter = Cursor<Vec<u8>>;

const STOCK_WIDTH: f64 = 45.0;
const STOCK_HEIGHT: f64 = 35.0;

macro_rules! import_err(
    ($code:tt, $str:expr) => {{
        use simlin_core::common::{Error, ErrorCode, ErrorKind};
        Err(Error::new(ErrorKind::Model, ErrorCode::$code, Some($str)))
    }}
);

const XMILE_VERSION: &str = "1.0";
// const NS_HTTPS: &str = "https://docs.oasis-open.org/xmile/ns/XMILE/v1.0";
const XML_NS_HTTP: &str = "http://docs.oasis-open.org/xmile/ns/XMILE/v1.0";
const XML_NS_ISEE: &str = "http://iseesystems.com/XMILE";
const XML_NS_SIMLIN: &str = "https://simlin.com/XMILE/v1.0";
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
    pub ai_information: Option<AiInformation>,
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

impl ToXml<XmlWriter> for File {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        // xmile tag
        let attrs = &[
            ("version", self.version.as_str()),
            ("xmlns", self.namespace.as_str()),
            ("xmlns:isee", XML_NS_ISEE),
            ("xmlns:simlin", XML_NS_SIMLIN),
        ];
        write_tag_start_with_attrs(writer, "xmile", attrs)?;

        if let Some(ref header) = self.header {
            header.write_xml(writer)?;
        }

        if let Some(ref sim_specs) = self.sim_specs {
            sim_specs.write_xml(writer)?;
        }

        if let Some(Units {
            unit: Some(ref units),
        }) = self.units
        {
            write_tag_start(writer, "model_units")?;
            for unit in units.iter() {
                unit.write_xml(writer)?;
            }
            write_tag_end(writer, "model_units")?;
        }

        if let Some(Dimensions {
            dimensions: Some(ref dimensions),
            ..
        }) = self.dimensions
        {
            write_tag_start(writer, "dimensions")?;
            for dim in dimensions.iter() {
                dim.write_xml(writer)?;
            }
            write_tag_end(writer, "dimensions")?;
        }

        for model in self.models.iter() {
            model.write_xml(writer)?;
        }

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
            units: match file.units {
                None => vec![],
                Some(units) => units
                    .unit
                    .unwrap_or_default()
                    .into_iter()
                    .map(datamodel::Unit::from)
                    .collect(),
            },
            models: file
                .models
                .into_iter()
                .map(datamodel::Model::from)
                .collect(),
            source: None,
            ai_information: file.ai_information.map(|ai| datamodel::AiInformation {
                status: datamodel::AiStatus {
                    key_url: ai.status.key_url,
                    algorithm: ai.status.algorithm,
                    signature: ai.status.signature,
                    tags: ai.status.tags,
                },
                testing: ai.testing.map(|t| datamodel::AiTesting {
                    signed_message_body: t.signed_message_body,
                }),
                log: ai.log,
            }),
        }
    }
}

impl From<datamodel::Project> for File {
    fn from(project: datamodel::Project) -> Self {
        File {
            version: XMILE_VERSION.to_owned(),
            namespace: XML_NS_HTTP.to_owned(),
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
            ai_information: None,
            sim_specs: Some(project.sim_specs.into()),
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
            units: if project.units.is_empty() {
                None
            } else {
                Some(Units {
                    unit: Some(project.units.into_iter().map(Unit::from).collect()),
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
pub struct AiInformation {
    pub status: AiStatus,
    pub testing: Option<AiTesting>,
    pub log: Option<String>,
    // TODO: settings
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct AiStatus {
    pub key_url: String,
    pub algorithm: String,
    pub signature: String,
    pub tags: HashMap<String, String>,
}

impl<'de> Deserialize<'de> for AiStatus {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StatusVisitor;

        impl<'de> Visitor<'de> for StatusVisitor {
            type Value = AiStatus;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an AiStatus with attributes")
            }

            fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut key_url = None;
                let mut algorithm = None;
                let mut signature = None;
                let mut tags = HashMap::new();

                while let Some((key, value)) = map.next_entry::<String, String>()? {
                    match key.as_str() {
                        "@keyurl" => key_url = Some(value),
                        "@algorithm" => algorithm = Some(value),
                        "@signature" => signature = Some(value),
                        k if k.starts_with('@') => {
                            // Remove @ prefix for the tags map
                            tags.insert(k[1..].to_string(), value);
                        }
                        _ => {
                            // Handle non-attribute fields if needed
                            tags.insert(key, value);
                        }
                    }
                }

                Ok(AiStatus {
                    key_url: key_url.ok_or_else(|| serde::de::Error::missing_field("keyurl"))?,
                    algorithm: algorithm
                        .ok_or_else(|| serde::de::Error::missing_field("algorithm"))?,
                    signature: signature
                        .ok_or_else(|| serde::de::Error::missing_field("signature"))?,
                    tags,
                })
            }
        }

        deserializer.deserialize_map(StatusVisitor)
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct AiTesting {
    #[serde(rename = "@signed_message_body")]
    pub signed_message_body: String,
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
    #[serde(rename = "@name")]
    pub name: String,
}

impl ToXml<XmlWriter> for VarDimension {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        let attrs = &[("name", self.name.as_ref())];
        write_tag_with_attrs(writer, "dim", "", attrs)
    }
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

fn xml_error(err: std::io::Error) -> simlin_core::common::Error {
    use simlin_core::common::{Error, ErrorCode, ErrorKind};

    Error::new(
        ErrorKind::Import,
        ErrorCode::XmlDeserialization,
        Some(err.to_string()),
    )
}

fn write_tag_start(writer: &mut Writer<XmlWriter>, tag_name: &str) -> Result<()> {
    write_tag_start_with_attrs(writer, tag_name, &[])
}

fn write_tag_start_with_attrs(
    writer: &mut Writer<XmlWriter>,
    tag_name: &str,
    attrs: &[(&str, &str)],
) -> Result<()> {
    let mut elem = BytesStart::new(tag_name);
    for attr in attrs.iter() {
        elem.push_attribute(*attr);
    }
    writer.write_event(Event::Start(elem)).map_err(xml_error)
}

fn write_tag_end(writer: &mut Writer<XmlWriter>, tag_name: &str) -> Result<()> {
    writer
        .write_event(Event::End(BytesEnd::new(tag_name)))
        .map_err(xml_error)
}

fn write_tag_text(writer: &mut Writer<XmlWriter>, content: &str) -> Result<()> {
    writer
        .write_event(Event::Text(BytesText::new(content)))
        .map_err(xml_error)
}

fn write_tag(writer: &mut Writer<XmlWriter>, tag_name: &str, content: &str) -> Result<()> {
    write_tag_with_attrs(writer, tag_name, content, &[])
}

fn write_tag_with_attrs(
    writer: &mut Writer<XmlWriter>,
    tag_name: &str,
    content: &str,
    attrs: &[(&str, &str)],
) -> Result<()> {
    write_tag_start_with_attrs(writer, tag_name, attrs)?;

    write_tag_text(writer, content)?;

    write_tag_end(writer, tag_name)
}

impl ToXml<XmlWriter> for Header {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
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
    #[serde(rename = "@save_interval")]
    pub save_step: Option<f64>,
    #[serde(rename = "@method")]
    pub method: Option<String>,
    #[serde(rename = "@time_units")]
    pub time_units: Option<String>,
}

impl ToXml<XmlWriter> for SimSpecs {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        let mut elem = BytesStart::new("sim_specs");
        if let Some(ref method) = self.method {
            elem.push_attribute(("method", method.as_str()));
        }
        if let Some(ref time_units) = self.time_units {
            elem.push_attribute(("time_units", time_units.as_str()));
        }
        if let Some(ref save_step) = self.save_step {
            let save_interval = format!("{save_step}");
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
            dt: sim_specs.dt.map(datamodel::Dt::from).unwrap_or_default(),
            save_step: sim_specs.save_step.map(datamodel::Dt::Dt),
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
    #[serde(rename = "@reciprocal")]
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
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@size")]
    pub size: Option<u32>,
    #[serde(rename = "elem")]
    pub elements: Option<Vec<Index>>,
    /// Vensim dimension mapping: when this dimension "maps to" another,
    /// elements correspond positionally (e.g., DimA -> DimB means A1<->B1, etc.)
    /// Note: Element in XML is <isee:maps_to> but quick-xml deserializes with local name only
    #[serde(rename = "maps_to")]
    pub maps_to: Option<String>,
}

impl ToXml<XmlWriter> for Dimension {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        let attrs = vec![("name", self.name.as_ref())];
        if self.size.is_some() {
            let size = format!("{}", self.size.unwrap());
            let mut attrs = attrs.clone();
            attrs.push(("size", size.as_str()));
            write_tag_start_with_attrs(writer, "dim", &attrs)?;
        } else {
            write_tag_start_with_attrs(writer, "dim", &attrs)?;
        }

        if let Some(ref elements) = self.elements {
            for element in elements.iter() {
                let attrs = &[("name", element.name.as_str())];
                write_tag_with_attrs(writer, "elem", "", attrs)?;
            }
        }

        // Write dimension mapping if present
        if let Some(ref maps_to) = self.maps_to {
            write_tag(writer, "isee:maps_to", maps_to)?;
        }

        write_tag_end(writer, "dim")
    }
}

impl From<Dimension> for datamodel::Dimension {
    fn from(dimension: Dimension) -> Self {
        let name = canonicalize(&dimension.name).as_str().to_string();
        let maps_to = dimension
            .maps_to
            .map(|m| canonicalize(&m).as_str().to_string());
        let elements = if let Some(elements) = dimension.elements {
            datamodel::DimensionElements::Named(
                elements
                    .into_iter()
                    .map(|i| canonicalize(&i.name).as_str().to_string())
                    .collect(),
            )
        } else {
            datamodel::DimensionElements::Indexed(dimension.size.unwrap_or_default())
        };
        datamodel::Dimension {
            name,
            elements,
            maps_to,
        }
    }
}

impl From<datamodel::Dimension> for Dimension {
    fn from(dimension: datamodel::Dimension) -> Self {
        match dimension.elements {
            datamodel::DimensionElements::Indexed(size) => Dimension {
                name: dimension.name,
                size: Some(size),
                elements: None,
                maps_to: dimension.maps_to,
            },
            datamodel::DimensionElements::Named(elements) => Dimension {
                name: dimension.name,
                size: None,
                elements: Some(elements.into_iter().map(|i| Index { name: i }).collect()),
                maps_to: dimension.maps_to,
            },
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Index {
    #[serde(rename = "@name")]
    pub name: String,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct GraphicalFunctionScale {
    #[serde(rename = "@min")]
    pub min: f64,
    #[serde(rename = "@max")]
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
pub struct Gf {
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

impl ToXml<XmlWriter> for Gf {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        let mut elem = BytesStart::new("gf");
        if let Some(ref name) = self.name {
            elem.push_attribute(("name", name.as_str()));
        }
        if let Some(ref kind) = self.kind {
            match kind {
                GraphicalFunctionKind::Continuous => {
                    // default, so don't write anything
                }
                GraphicalFunctionKind::Extrapolate => elem.push_attribute(("type", "extrapolate")),
                GraphicalFunctionKind::Discrete => elem.push_attribute(("type", "discrete")),
            }
        }
        writer.write_event(Event::Start(elem)).map_err(xml_error)?;

        if let Some(ref x_scale) = self.x_scale {
            let min = format!("{}", x_scale.min);
            let max = format!("{}", x_scale.max);
            let attrs = &[("min", min.as_str()), ("max", max.as_str())];
            write_tag_start_with_attrs(writer, "xscale", attrs)?;
            write_tag_end(writer, "xscale")?;
        }

        if let Some(ref y_scale) = self.y_scale {
            let min = format!("{}", y_scale.min);
            let max = format!("{}", y_scale.max);
            let attrs = &[("min", min.as_str()), ("max", max.as_str())];
            write_tag_start_with_attrs(writer, "yscale", attrs)?;
            write_tag_end(writer, "yscale")?;
        }

        if let Some(ref x_pts) = self.x_pts {
            write_tag(writer, "xpts", x_pts)?;
        }

        if let Some(ref y_pts) = self.y_pts {
            write_tag(writer, "ypts", y_pts)?;
        }

        write_tag_end(writer, "gf")
    }
}

impl From<Gf> for datamodel::GraphicalFunction {
    fn from(gf: Gf) -> Self {
        use std::str::FromStr;

        let kind = datamodel::GraphicalFunctionKind::from(
            gf.kind.unwrap_or(GraphicalFunctionKind::Continuous),
        );

        let x_points: std::result::Result<Vec<f64>, _> = match &gf.x_pts {
            None => Ok(vec![]),
            Some(x_pts) => x_pts.split(',').map(|n| f64::from_str(n.trim())).collect(),
        };
        let x_points: Vec<f64> = x_points.unwrap_or_default();

        let y_points: std::result::Result<Vec<f64>, _> = match &gf.y_pts {
            None => Ok(vec![]),
            Some(y_pts) => y_pts.split(',').map(|n| f64::from_str(n.trim())).collect(),
        };
        let y_points: Vec<f64> = y_points.unwrap_or_default();

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

impl From<datamodel::GraphicalFunction> for Gf {
    fn from(gf: datamodel::GraphicalFunction) -> Self {
        let x_pts: Option<String> = gf.x_points.map(|x_points| {
            x_points
                .into_iter()
                .map(|f| f.to_string())
                .collect::<Vec<String>>()
                .join(",")
        });
        let y_pts = gf
            .y_points
            .into_iter()
            .map(|f| f.to_string())
            .collect::<Vec<String>>()
            .join(",");
        Gf {
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
    #[serde(rename = "@name")]
    pub name: String,
    pub eqn: Option<String>,
    pub alias: Option<Vec<String>>,
    pub disabled: Option<bool>,
}

impl ToXml<XmlWriter> for Unit {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        let mut attrs = vec![("name", self.name.as_str())];
        if matches!(self.disabled, Some(true)) {
            attrs.push(("disabled", "true"));
        }
        write_tag_start_with_attrs(writer, "unit", &attrs)?;

        if let Some(ref eqn) = self.eqn {
            write_tag(writer, "eqn", eqn)?;
        }

        if let Some(ref aliases) = self.alias {
            for alias in aliases.iter() {
                write_tag(writer, "alias", alias)?;
            }
        }

        write_tag_end(writer, "unit")
    }
}

impl From<datamodel::Unit> for Unit {
    fn from(unit: datamodel::Unit) -> Self {
        Unit {
            name: unit.name,
            eqn: unit.equation,
            disabled: if unit.disabled { Some(true) } else { None },
            alias: if unit.aliases.is_empty() {
                None
            } else {
                Some(unit.aliases)
            },
        }
    }
}

impl From<Unit> for datamodel::Unit {
    fn from(unit: Unit) -> Self {
        datamodel::Unit {
            name: unit.name,
            equation: unit.eqn.filter(|eqn| !eqn.is_empty()),
            disabled: matches!(unit.disabled, Some(true)),
            aliases: unit.alias.unwrap_or_default(),
        }
    }
}

#[test]
fn test_unit_roundtrip() {
    let cases: &[_] = &[
        datamodel::Unit {
            name: "people".to_string(),
            equation: None,
            disabled: false,
            aliases: vec!["peoples".to_owned(), "person".to_owned()],
        },
        datamodel::Unit {
            name: "cows".to_string(),
            equation: Some("1/people".to_owned()),
            disabled: true,
            aliases: vec![],
        },
    ];
    for expected in cases {
        let expected = expected.clone();
        let actual = datamodel::Unit::from(Unit::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Model {
    #[serde(rename = "@name", default)]
    pub name: Option<String>,
    #[serde(rename = "namespace")]
    pub namespaces: Option<String>, // comma separated list of namespaces
    pub resource: Option<String>, // path or URL to separate resource file
    pub sim_specs: Option<SimSpecs>,
    pub variables: Option<Variables>,
    pub views: Option<Views>,
}

impl ToXml<XmlWriter> for Model {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        if self.name.is_none() || self.name.as_ref().unwrap() == "main" {
            write_tag_start(writer, "model")?;
        } else {
            let attrs = &[("name", self.name.as_deref().unwrap())];
            write_tag_start_with_attrs(writer, "model", attrs)?;
        }

        write_tag_start(writer, "variables")?;

        if let Some(Variables { ref variables }) = self.variables {
            for var in variables.iter() {
                var.write_xml(writer)?;
            }
        }

        write_tag_end(writer, "variables")?;

        write_tag_start(writer, "views")?;

        if let Some(Views {
            view: Some(ref views),
            ..
        }) = self.views
        {
            for view in views.iter() {
                view.write_xml(writer)?;
            }
        }

        write_tag_end(writer, "views")?;

        write_tag_end(writer, "model")
    }
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
            name: model.name.as_deref().unwrap_or("main").to_string(),
            sim_specs: model.sim_specs.map(datamodel::SimSpecs::from),
            variables: match model.variables {
                Some(Variables {
                    variables: vars, ..
                }) => {
                    let mut variables: Vec<datamodel::Variable> = vars
                        .into_iter()
                        .filter(|v| !matches!(v, Var::Unhandled))
                        .map(datamodel::Variable::from)
                        .collect();
                    // Sort variables by canonical identifier for deterministic ordering
                    variables.sort_by(|a, b| {
                        simlin_core::canonicalize(a.get_ident())
                            .cmp(&simlin_core::canonicalize(b.get_ident()))
                    });
                    variables
                }
                _ => vec![],
            },
            views,
            loop_metadata: vec![],
        }
    }
}

impl From<datamodel::Model> for Model {
    fn from(model: datamodel::Model) -> Self {
        let datamodel::Model {
            name,
            sim_specs,
            variables,
            views,
            ..
        } = model;
        Model {
            name: Some(name),
            namespaces: None,
            resource: None,
            sim_specs: sim_specs.map(SimSpecs::from),
            variables: if variables.is_empty() {
                None
            } else {
                let variables = variables.into_iter().map(Var::from).collect();
                Some(Variables { variables })
            },
            views: if views.is_empty() {
                None
            } else {
                Some(Views {
                    view: Some(views.into_iter().map(View::from).collect()),
                })
            },
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct Variables {
    #[serde(rename = "$value", default)]
    pub variables: Vec<Var>,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct Views {
    pub view: Option<Vec<View>>,
}

impl Model {
    pub fn get_name(&self) -> &str {
        self.name.as_deref().unwrap_or("main")
    }

    // TODO: if this is a bottleneck, we should have a normalize pass over
    //   the model to canonicalize things once (and build a map)
    pub fn get_var(&self, ident: &str) -> Option<&Var> {
        self.variables.as_ref()?;

        for var in self.variables.as_ref().unwrap().variables.iter() {
            let name = var.get_noncanonical_name();
            if ident == name || ident == canonicalize(name).as_str() {
                return Some(var);
            }
        }

        None
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewType {
    StockFlow,
    Interface,
    Popup,
    VendorSpecific,
}

impl ViewType {
    fn as_str(&self) -> &'static str {
        match self {
            ViewType::StockFlow => "stock_flow",
            ViewType::Interface => "interface",
            ViewType::Popup => "popup",
            ViewType::VendorSpecific => "vendor_specific",
        }
    }
}

pub mod view_element {
    use super::datamodel;
    use crate::xmile::{
        STOCK_HEIGHT, STOCK_WIDTH, ToXml, XmlWriter, write_tag, write_tag_end, write_tag_start,
        write_tag_start_with_attrs, write_tag_text, write_tag_with_attrs,
    };
    use quick_xml::Writer;
    use serde::{Deserialize, Deserializer, Serialize};
    use simlin_core::common::Result;
    #[cfg(test)]
    use simlin_core::datamodel::StockFlow;
    use simlin_core::datamodel::view_element::LinkShape;

    /// Normalize an angle to the range [0, 360).
    /// Use this to sanitize angles read from XMILE files before conversion.
    fn normalize_angle(degrees: f64) -> f64 {
        let normalized = degrees % 360.0;
        if normalized < 0.0 {
            normalized + 360.0
        } else {
            normalized
        }
    }

    /// Convert an angle from XMILE format [0, 360) to canvas format [-180, 180].
    /// XMILE uses counter-clockwise with Y-up; canvas uses Y-down.
    fn convert_angle_from_xmile_to_canvas(in_degrees: f64) -> f64 {
        let out_degrees = (360.0 - in_degrees) % 360.0;
        if out_degrees > 180.0 {
            out_degrees - 360.0
        } else {
            out_degrees
        }
    }

    /// Convert an angle from canvas format [-180, 180] to XMILE format [0, 360).
    fn convert_angle_from_canvas_to_xmile(in_degrees: f64) -> f64 {
        let out_degrees = if in_degrees < 0.0 {
            in_degrees + 360.0
        } else {
            in_degrees
        };
        (360.0 - out_degrees) % 360.0
    }

    /// Get the position (x, y) of a view element by its uid.
    fn get_element_position(view: &datamodel::StockFlow, uid: i32) -> Option<(f64, f64)> {
        for element in &view.elements {
            match element {
                datamodel::ViewElement::Aux(e) if e.uid == uid => return Some((e.x, e.y)),
                datamodel::ViewElement::Stock(e) if e.uid == uid => return Some((e.x, e.y)),
                datamodel::ViewElement::Flow(e) if e.uid == uid => return Some((e.x, e.y)),
                datamodel::ViewElement::Module(e) if e.uid == uid => return Some((e.x, e.y)),
                datamodel::ViewElement::Alias(e) if e.uid == uid => return Some((e.x, e.y)),
                datamodel::ViewElement::Cloud(e) if e.uid == uid => return Some((e.x, e.y)),
                _ => {}
            }
        }
        None
    }

    /// Calculate the straight-line angle (in canvas coordinates, degrees) between two points.
    /// Returns the angle from (from_x, from_y) to (to_x, to_y) in [-180, 180] range.
    fn calculate_straight_line_angle(from_x: f64, from_y: f64, to_x: f64, to_y: f64) -> f64 {
        let dx = to_x - from_x;
        let dy = to_y - from_y;
        dy.atan2(dx).to_degrees()
    }

    /// Epsilon for comparing angles - angles within this threshold are considered equal.
    /// This is tight to ensure roundtrip fidelity (original angles are preserved).
    const ANGLE_EPSILON_DEGREES: f64 = 0.01;

    /// Check if an angle (in canvas coordinates) is effectively equal to the straight-line
    /// angle between two points. Uses a tight epsilon to ensure roundtrip fidelity.
    fn is_straight_line_angle(
        angle_degrees: f64,
        from_x: f64,
        from_y: f64,
        to_x: f64,
        to_y: f64,
    ) -> bool {
        let straight_angle = calculate_straight_line_angle(from_x, from_y, to_x, to_y);
        let diff = (angle_degrees - straight_angle).abs();
        // Handle wraparound (e.g., -179 vs 179 should be close)
        let diff = if diff > 180.0 { 360.0 - diff } else { diff };
        diff < ANGLE_EPSILON_DEGREES
    }

    #[test]
    fn test_normalize_angle() {
        // Already in range
        assert_eq!(0.0, normalize_angle(0.0));
        assert_eq!(45.0, normalize_angle(45.0));
        assert_eq!(359.0, normalize_angle(359.0));

        // Negative angles
        assert_eq!(315.0, normalize_angle(-45.0));
        assert_eq!(270.0, normalize_angle(-90.0));
        assert_eq!(180.0, normalize_angle(-180.0));
        assert_eq!(1.0, normalize_angle(-359.0));

        // Angles >= 360
        assert_eq!(0.0, normalize_angle(360.0));
        assert_eq!(45.0, normalize_angle(405.0));
        assert_eq!(90.0, normalize_angle(450.0));

        // Large negative
        assert_eq!(320.0, normalize_angle(-400.0));
    }

    #[test]
    fn test_convert_angles() {
        let cases: &[(f64, f64)] = &[(0.0, 0.0), (45.0, -45.0), (270.0, 90.0)];

        for (xmile, canvas) in cases {
            assert_eq!(*canvas, convert_angle_from_xmile_to_canvas(*xmile));
            assert_eq!(*xmile, convert_angle_from_canvas_to_xmile(*canvas));
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

    impl LabelSide {
        fn as_str(&self) -> &'static str {
            match self {
                LabelSide::Top => "top",
                LabelSide::Left => "left",
                LabelSide::Center => "center",
                LabelSide::Bottom => "bottom",
                LabelSide::Right => "right",
            }
        }
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
            let expected = *expected;
            let actual = datamodel::view_element::LabelSide::from(LabelSide::from(expected));
            assert_eq!(expected, actual);
        }
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Aux {
        #[serde(rename = "@name")]
        pub name: String,
        #[serde(rename = "@uid")]
        pub uid: Option<i32>,
        #[serde(rename = "@x")]
        pub x: f64,
        #[serde(rename = "@y")]
        pub y: f64,
        #[serde(rename = "@width")]
        pub width: Option<f64>,
        #[serde(rename = "@height")]
        pub height: Option<f64>,
        #[serde(rename = "@label_side")]
        pub label_side: Option<LabelSide>,
        #[serde(rename = "@label_angle")]
        pub label_angle: Option<f64>,
    }

    impl ToXml<XmlWriter> for Aux {
        fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
            let x = format!("{}", self.x);
            let y = format!("{}", self.y);
            let label_side = self.label_side.map(|side| side.as_str());

            let mut attrs = vec![
                ("name", self.name.as_str()),
                ("x", x.as_str()),
                ("y", y.as_str()),
            ];
            if let Some(label_side) = label_side {
                attrs.push(("label_side", label_side));
            }
            write_tag_with_attrs(writer, "aux", "", &attrs)
        }
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
        #[serde(rename = "@name")]
        pub name: String,
        #[serde(rename = "@uid")]
        pub uid: Option<i32>,
        #[serde(rename = "@x")]
        pub x: f64,
        #[serde(rename = "@y")]
        pub y: f64,
        #[serde(rename = "@width")]
        pub width: Option<f64>,
        #[serde(rename = "@height")]
        pub height: Option<f64>,
        #[serde(rename = "@label_side")]
        pub label_side: Option<LabelSide>,
        #[serde(rename = "@label_angle")]
        pub label_angle: Option<f64>,
    }

    impl ToXml<XmlWriter> for Stock {
        fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
            let x = format!("{}", self.x);
            let y = format!("{}", self.y);
            let label_side = self.label_side.map(|side| side.as_str());

            let mut attrs = vec![
                ("name", self.name.as_str()),
                ("x", x.as_str()),
                ("y", y.as_str()),
            ];
            if let Some(label_side) = label_side {
                attrs.push(("label_side", label_side));
            }
            write_tag_with_attrs(writer, "stock", "", &attrs)
        }
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
        #[serde(rename = "@x")]
        pub x: f64,
        #[serde(rename = "@y")]
        pub y: f64,
        #[serde(rename = "@uid")]
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
        #[serde(rename = "@name")]
        pub name: String,
        #[serde(rename = "@uid")]
        pub uid: Option<i32>,
        #[serde(rename = "@x")]
        pub x: f64,
        #[serde(rename = "@y")]
        pub y: f64,
        #[serde(rename = "@width")]
        pub width: Option<f64>,
        #[serde(rename = "@height")]
        pub height: Option<f64>,
        #[serde(rename = "@label_side")]
        pub label_side: Option<LabelSide>,
        #[serde(rename = "@label_angle")]
        pub label_angle: Option<f64>,
        #[serde(rename = "pts")]
        pub points: Option<Points>,
    }

    impl ToXml<XmlWriter> for Flow {
        fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
            let x = format!("{}", self.x);
            let y = format!("{}", self.y);
            let label_side = self.label_side.map(|side| side.as_str());

            let mut attrs = vec![
                ("name", self.name.as_str()),
                ("x", x.as_str()),
                ("y", y.as_str()),
            ];
            if let Some(label_side) = label_side {
                attrs.push(("label_side", label_side));
            }
            write_tag_start_with_attrs(writer, "flow", &attrs)?;

            if self.points.is_some() && !self.points.as_ref().unwrap().points.is_empty() {
                write_tag_start(writer, "pts")?;
                for point in self.points.as_ref().unwrap().points.iter() {
                    let x = format!("{}", point.x);
                    let y = format!("{}", point.y);
                    let attrs = &[("x", x.as_str()), ("y", y.as_str())];
                    write_tag_with_attrs(writer, "pt", "", attrs)?;
                }
                write_tag_end(writer, "pts")?;
            }

            write_tag_end(writer, "flow")
        }
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
        let actual_v = datamodel::view_element::Flow::from(Flow::from(input_v));
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
        let actual_h = datamodel::view_element::Flow::from(Flow::from(input_h));
        assert_eq!(expected_h, actual_h);
    }

    #[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
    pub struct AliasLinkEnd {
        #[serde(rename = "@uid")]
        pub uid: i32,
    }

    #[derive(Clone, PartialEq, Eq, Debug, Serialize)]
    pub enum LinkEnd {
        #[serde(rename = "$value")]
        Named(String),
        #[serde(rename = "alias")]
        Alias(AliasLinkEnd),
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Link {
        #[serde(rename = "@uid")]
        pub uid: Option<i32>,
        #[serde(deserialize_with = "deserialize_link_end")]
        pub from: LinkEnd,
        #[serde(rename = "@from_uid")]
        pub from_uid: Option<i32>,
        #[serde(deserialize_with = "deserialize_link_end")]
        pub to: LinkEnd,
        #[serde(rename = "@to_uid")]
        pub to_uid: Option<i32>,
        #[serde(rename = "@angle")]
        pub angle: Option<f64>,
        #[serde(rename = "@is_straight")]
        pub is_straight: Option<bool>,
        #[serde(rename = "pts")]
        pub points: Option<Points>, // for multi-point connectors
    }

    fn deserialize_link_end<'de, D>(deserializer: D) -> std::result::Result<LinkEnd, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct LinkEndInner {
            #[serde(rename = "$value", default)]
            named: String,
            alias: Option<AliasLinkEnd>,
        }
        let inner = LinkEndInner::deserialize(deserializer)?;
        if let Some(alias) = inner.alias {
            Ok(LinkEnd::Alias(alias))
        } else {
            Ok(LinkEnd::Named(inner.named))
        }
    }

    impl ToXml<XmlWriter> for Link {
        fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
            let angle = self.angle.map(|angle| format!("{angle}"));

            let mut attrs = Vec::with_capacity(1);
            if let Some(ref angle) = angle {
                attrs.push(("angle", angle.as_str()));
            }
            write_tag_start_with_attrs(writer, "connector", &attrs)?;

            write_tag_start(writer, "from")?;
            match self.from {
                LinkEnd::Named(ref name) => {
                    write_tag_text(writer, name)?;
                }
                LinkEnd::Alias(ref uid) => {
                    let uid = format!("{}", uid.uid);
                    let attrs = &[("uid", uid.as_str())];
                    write_tag_with_attrs(writer, "alias", "", attrs)?;
                }
            }
            write_tag_end(writer, "from")?;

            write_tag_start(writer, "to")?;
            match self.to {
                LinkEnd::Named(ref name) => {
                    write_tag_text(writer, name)?;
                }
                LinkEnd::Alias(ref uid) => {
                    let uid = format!("{}", uid.uid);
                    let attrs = &[("uid", uid.as_str())];
                    write_tag_with_attrs(writer, "alias", "", attrs)?;
                }
            }
            write_tag_end(writer, "to")?;

            if self.points.is_some() && !self.points.as_ref().unwrap().points.is_empty() {
                write_tag_start(writer, "pts")?;
                for point in self.points.as_ref().unwrap().points.iter() {
                    let x = format!("{}", point.x);
                    let y = format!("{}", point.y);
                    let attrs = &[("x", x.as_str()), ("y", y.as_str())];
                    write_tag_with_attrs(writer, "pt", "", attrs)?;
                }
                write_tag_end(writer, "pts")?;
            }

            write_tag_end(writer, "connector")
        }
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
                // Normalize XMILE angle to [0, 360), then convert to canvas format for internal use
                let xmile_angle = normalize_angle(v.angle.unwrap_or(0.0));
                datamodel::view_element::LinkShape::Arc(convert_angle_from_xmile_to_canvas(
                    xmile_angle,
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

    /// Convert from an XMILE Link with access to a position map for lookup.
    /// This detects straight lines by comparing the angle to the direct from->to angle.
    pub(super) fn link_from_xmile_with_positions(
        v: Link,
        positions: &std::collections::HashMap<i32, (f64, f64)>,
    ) -> datamodel::view_element::Link {
        let from_uid = v.from_uid.unwrap_or(-1);
        let to_uid = v.to_uid.unwrap_or(-1);

        let shape = if v.is_straight.unwrap_or(false) {
            // Explicit is_straight flag
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
        } else if let Some(angle) = v.angle {
            // Normalize XMILE angle to [0, 360), then convert to canvas format
            let xmile_angle = normalize_angle(angle);
            let canvas_angle = convert_angle_from_xmile_to_canvas(xmile_angle);
            // Check if this angle represents a straight line (comparison in canvas coords)
            if let (Some(&(from_x, from_y)), Some(&(to_x, to_y))) =
                (positions.get(&from_uid), positions.get(&to_uid))
            {
                if is_straight_line_angle(canvas_angle, from_x, from_y, to_x, to_y) {
                    datamodel::view_element::LinkShape::Straight
                } else {
                    datamodel::view_element::LinkShape::Arc(canvas_angle)
                }
            } else {
                // Can't look up positions, treat as arc
                datamodel::view_element::LinkShape::Arc(canvas_angle)
            }
        } else {
            // No angle specified, default to arc at 0 (canvas format)
            datamodel::view_element::LinkShape::Arc(0.0)
        };

        datamodel::view_element::Link {
            uid: v.uid.unwrap_or(-1),
            from_uid,
            to_uid,
            shape,
        }
    }

    /// Convert from an XMILE Link with access to the view for position lookup.
    /// This is a convenience wrapper around link_from_xmile_with_positions for tests.
    #[cfg(test)]
    fn link_from_xmile_with_view(
        v: Link,
        view: &datamodel::StockFlow,
    ) -> datamodel::view_element::Link {
        let positions: std::collections::HashMap<i32, (f64, f64)> = view
            .elements
            .iter()
            .filter_map(|e| {
                let uid = e.get_uid();
                get_element_position(view, uid).map(|pos| (uid, pos))
            })
            .collect();
        link_from_xmile_with_positions(v, &positions)
    }

    impl Link {
        pub fn from(v: datamodel::view_element::Link, view: &datamodel::StockFlow) -> Self {
            let (is_straight, angle, points) = match v.shape {
                LinkShape::Straight => {
                    // Calculate the straight-line angle from element positions so other
                    // SD software (like Stella) can read the XMILE file correctly.
                    if let (Some((from_x, from_y)), Some((to_x, to_y))) = (
                        get_element_position(view, v.from_uid),
                        get_element_position(view, v.to_uid),
                    ) {
                        // Calculate in canvas coords, convert to XMILE format
                        let canvas_angle =
                            calculate_straight_line_angle(from_x, from_y, to_x, to_y);
                        let xmile_angle =
                            normalize_angle(convert_angle_from_canvas_to_xmile(canvas_angle));
                        (None, Some(xmile_angle), None)
                    } else {
                        // Fallback if positions aren't found
                        (Some(true), None, None)
                    }
                }
                LinkShape::Arc(canvas_angle) => {
                    // Convert from internal canvas format to XMILE format, normalized to [0, 360)
                    let xmile_angle =
                        normalize_angle(convert_angle_from_canvas_to_xmile(canvas_angle));
                    (None, Some(xmile_angle), None)
                }
                LinkShape::MultiPoint(points) => (
                    None,
                    None,
                    Some(Points {
                        points: points.into_iter().map(Point::from).collect(),
                    }),
                ),
            };
            let from_name = view.get_variable_name(v.from_uid).unwrap_or("");
            let to_name = view.get_variable_name(v.to_uid).unwrap_or("");
            Link {
                uid: Some(v.uid),
                from: if from_name.is_empty() {
                    LinkEnd::Alias(AliasLinkEnd { uid: v.from_uid })
                } else {
                    LinkEnd::Named(from_name.to_owned())
                },
                from_uid: Some(v.from_uid),
                to: if to_name.is_empty() {
                    LinkEnd::Alias(AliasLinkEnd { uid: v.to_uid })
                } else {
                    LinkEnd::Named(to_name.to_owned())
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
        // Internal angles are in canvas format [-180, 180]
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
                shape: LinkShape::Arc(-45.0), // canvas format
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
        let view = StockFlow {
            elements: vec![],
            view_box: Default::default(),
            zoom: 0.0,
        };
        for expected in cases {
            let expected = expected.clone();
            let actual = datamodel::view_element::Link::from(Link::from(expected.clone(), &view));
            assert_eq!(expected, actual);
        }
    }

    #[test]
    fn test_straight_link_export_calculates_angle() {
        // When exporting a LinkShape::Straight, we should calculate the angle
        // based on the from/to element positions so other software can read it.
        let view = StockFlow {
            elements: vec![
                datamodel::ViewElement::Aux(datamodel::view_element::Aux {
                    name: "from_var".to_string(),
                    uid: 1,
                    x: 0.0,
                    y: 0.0,
                    label_side: datamodel::view_element::LabelSide::Top,
                }),
                datamodel::ViewElement::Aux(datamodel::view_element::Aux {
                    name: "to_var".to_string(),
                    uid: 2,
                    x: 100.0,
                    y: 0.0, // directly to the right
                    label_side: datamodel::view_element::LabelSide::Top,
                }),
            ],
            view_box: Default::default(),
            zoom: 1.0,
        };

        let link = datamodel::view_element::Link {
            uid: 10,
            from_uid: 1,
            to_uid: 2,
            shape: LinkShape::Straight,
        };

        let xmile_link = Link::from(link, &view);

        // For a horizontal right-pointing link, the angle should be 0 degrees
        // in canvas coordinates (the format used in xmile::Link)
        assert!(
            xmile_link.angle.is_some(),
            "straight link should export with an angle"
        );
        assert!(
            (xmile_link.angle.unwrap() - 0.0).abs() < 0.001,
            "horizontal right link should have angle ~0, got {}",
            xmile_link.angle.unwrap()
        );
        assert!(
            xmile_link.is_straight.is_none(),
            "should not set is_straight when exporting (for compatibility)"
        );
    }

    #[test]
    fn test_straight_link_export_diagonal() {
        // Test a diagonal link (down and to the right in screen coords)
        let view = StockFlow {
            elements: vec![
                datamodel::ViewElement::Aux(datamodel::view_element::Aux {
                    name: "from_var".to_string(),
                    uid: 1,
                    x: 0.0,
                    y: 0.0,
                    label_side: datamodel::view_element::LabelSide::Top,
                }),
                datamodel::ViewElement::Aux(datamodel::view_element::Aux {
                    name: "to_var".to_string(),
                    uid: 2,
                    x: 100.0,
                    y: 100.0, // down and to the right (45 degrees in canvas coords, Y-down)
                    label_side: datamodel::view_element::LabelSide::Top,
                }),
            ],
            view_box: Default::default(),
            zoom: 1.0,
        };

        let link = datamodel::view_element::Link {
            uid: 10,
            from_uid: 1,
            to_uid: 2,
            shape: LinkShape::Straight,
        };

        let xmile_link = Link::from(link, &view);

        // Canvas angle is 45° (down-right, Y-down), which converts to XMILE 315° (Y-up)
        assert!(xmile_link.angle.is_some());
        let angle = xmile_link.angle.unwrap();
        assert!(
            (angle - 315.0).abs() < 0.001,
            "diagonal down-right link should have XMILE angle ~315, got {}",
            angle
        );
    }

    #[test]
    fn test_straight_link_import_detects_straight() {
        // When importing an XMILE link whose angle exactly matches the straight-line
        // angle, we should convert to LinkShape::Straight
        let view = StockFlow {
            elements: vec![
                datamodel::ViewElement::Aux(datamodel::view_element::Aux {
                    name: "from_var".to_string(),
                    uid: 1,
                    x: 0.0,
                    y: 0.0,
                    label_side: datamodel::view_element::LabelSide::Top,
                }),
                datamodel::ViewElement::Aux(datamodel::view_element::Aux {
                    name: "to_var".to_string(),
                    uid: 2,
                    x: 100.0,
                    y: 0.0, // directly to the right
                    label_side: datamodel::view_element::LabelSide::Top,
                }),
            ],
            view_box: Default::default(),
            zoom: 1.0,
        };

        // Create an XMILE link with angle = 0 (straight horizontal right)
        let xmile_link = Link {
            uid: Some(10),
            from: LinkEnd::Named("from_var".to_string()),
            from_uid: Some(1),
            to: LinkEnd::Named("to_var".to_string()),
            to_uid: Some(2),
            angle: Some(0.0), // canvas coords: 0 degrees = pointing right
            is_straight: None,
            points: None,
        };

        let dm_link = link_from_xmile_with_view(xmile_link, &view);

        assert_eq!(
            dm_link.shape,
            LinkShape::Straight,
            "angle 0 for horizontal link should become LinkShape::Straight"
        );
    }

    #[test]
    fn test_curved_link_import_stays_curved() {
        // When importing an XMILE link whose angle differs significantly from
        // the straight-line angle, it should stay as LinkShape::Arc
        let view = StockFlow {
            elements: vec![
                datamodel::ViewElement::Aux(datamodel::view_element::Aux {
                    name: "from_var".to_string(),
                    uid: 1,
                    x: 0.0,
                    y: 0.0,
                    label_side: datamodel::view_element::LabelSide::Top,
                }),
                datamodel::ViewElement::Aux(datamodel::view_element::Aux {
                    name: "to_var".to_string(),
                    uid: 2,
                    x: 100.0,
                    y: 0.0, // directly to the right
                    label_side: datamodel::view_element::LabelSide::Top,
                }),
            ],
            view_box: Default::default(),
            zoom: 1.0,
        };

        // Create an XMILE link with angle = 45 (curved, not straight)
        // For a horizontal link, straight would be 0 degrees
        let xmile_link = Link {
            uid: Some(10),
            from: LinkEnd::Named("from_var".to_string()),
            from_uid: Some(1),
            to: LinkEnd::Named("to_var".to_string()),
            to_uid: Some(2),
            angle: Some(45.0), // significantly different from straight (0 degrees)
            is_straight: None,
            points: None,
        };

        let dm_link = link_from_xmile_with_view(xmile_link, &view);

        // Should stay as Arc, not Straight
        match dm_link.shape {
            LinkShape::Arc(angle) => {
                // XMILE 45° converts to canvas -45° (Y-axis flip)
                assert!(
                    (angle - (-45.0)).abs() < 0.001,
                    "expected arc angle ~-45 (canvas), got {}",
                    angle
                );
            }
            _ => panic!("expected LinkShape::Arc, got {:?}", dm_link.shape),
        }
    }

    #[test]
    fn test_straight_link_import_roundtrip_fidelity() {
        // For roundtrip fidelity, only angles that (nearly) exactly match the
        // calculated straight-line angle should become LinkShape::Straight.
        // Angles that are "close enough" for visual straightness (within 6 degrees)
        // but not exact should stay as Arc to preserve the original value.
        let view = StockFlow {
            elements: vec![
                datamodel::ViewElement::Aux(datamodel::view_element::Aux {
                    name: "from_var".to_string(),
                    uid: 1,
                    x: 0.0,
                    y: 0.0,
                    label_side: datamodel::view_element::LabelSide::Top,
                }),
                datamodel::ViewElement::Aux(datamodel::view_element::Aux {
                    name: "to_var".to_string(),
                    uid: 2,
                    x: 100.0,
                    y: 0.0,
                    label_side: datamodel::view_element::LabelSide::Top,
                }),
            ],
            view_box: Default::default(),
            zoom: 1.0,
        };

        // Angle very close to straight (within epsilon) should become Straight
        let nearly_exact = Link {
            uid: Some(10),
            from: LinkEnd::Named("from_var".to_string()),
            from_uid: Some(1),
            to: LinkEnd::Named("to_var".to_string()),
            to_uid: Some(2),
            angle: Some(0.005), // very close to 0 (straight horizontal)
            is_straight: None,
            points: None,
        };

        let dm_link_exact = link_from_xmile_with_view(nearly_exact, &view);
        assert_eq!(
            dm_link_exact.shape,
            LinkShape::Straight,
            "angle nearly exactly matching straight-line should become Straight"
        );

        // Angle slightly off (e.g., 5 degrees) should stay as Arc for roundtrip fidelity
        let slightly_off = Link {
            uid: Some(10),
            from: LinkEnd::Named("from_var".to_string()),
            from_uid: Some(1),
            to: LinkEnd::Named("to_var".to_string()),
            to_uid: Some(2),
            angle: Some(5.0), // 5 degrees from straight - visually straight but not exact
            is_straight: None,
            points: None,
        };

        let dm_link_off = link_from_xmile_with_view(slightly_off, &view);
        match dm_link_off.shape {
            LinkShape::Arc(_) => {} // expected - preserves original for roundtrip
            _ => panic!("angle not exactly matching should stay as Arc for roundtrip fidelity"),
        }
    }

    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Module {
        #[serde(rename = "@name")]
        pub name: String,
        #[serde(rename = "@uid")]
        pub uid: Option<i32>,
        #[serde(rename = "@x")]
        pub x: f64,
        #[serde(rename = "@y")]
        pub y: f64,
        #[serde(rename = "@label_side")]
        pub label_side: Option<LabelSide>,
    }

    impl ToXml<XmlWriter> for Module {
        fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
            let x = format!("{}", self.x);
            let y = format!("{}", self.y);
            let label_side = self.label_side.map(|side| side.as_str());

            let mut attrs = vec![
                ("name", self.name.as_str()),
                ("x", x.as_str()),
                ("y", y.as_str()),
            ];
            if let Some(label_side) = label_side {
                attrs.push(("label_side", label_side));
            }
            write_tag_with_attrs(writer, "module", "", &attrs)
        }
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
        #[serde(rename = "@of_uid")]
        pub of_uid: Option<i32>,
        #[serde(rename = "@uid")]
        pub uid: Option<i32>,
        #[serde(rename = "@x")]
        pub x: f64,
        #[serde(rename = "@y")]
        pub y: f64,
        #[serde(rename = "@label_side")]
        pub label_side: Option<LabelSide>,
    }

    impl ToXml<XmlWriter> for Alias {
        fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
            let uid = self.uid.map(|uid| format!("{uid}"));
            let x = format!("{}", self.x);
            let y = format!("{}", self.y);
            let label_side = self.label_side.map(|side| side.as_str());

            let mut attrs = vec![("x", x.as_str()), ("y", y.as_str())];
            if let Some(ref uid) = uid {
                attrs.push(("uid", uid.as_str()));
            }
            if let Some(label_side) = label_side {
                attrs.push(("label_side", label_side));
            }
            write_tag_start_with_attrs(writer, "alias", &attrs)?;

            write_tag(writer, "of", self.of.as_str())?;

            write_tag_end(writer, "alias")
        }
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

    impl Alias {
        pub fn from(v: datamodel::view_element::Alias, view: &datamodel::StockFlow) -> Self {
            Alias {
                uid: Some(v.uid),
                of: view
                    .get_variable_name(v.alias_of_uid)
                    .unwrap_or("")
                    .to_owned(),
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
        let view = StockFlow {
            elements: vec![],
            view_box: Default::default(),
            zoom: 0.0,
        };
        for expected in cases {
            let expected = expected.clone();
            let actual = datamodel::view_element::Alias::from(Alias::from(expected.clone(), &view));
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

    /// Visual container for grouping related model elements.
    /// In XMILE, x/y are top-left coordinates.
    #[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
    pub struct Group {
        #[serde(rename = "@name")]
        pub name: String,
        #[serde(rename = "@uid")]
        pub uid: Option<i32>,
        #[serde(rename = "@x")]
        pub x: f64,
        #[serde(rename = "@y")]
        pub y: f64,
        #[serde(rename = "@width")]
        pub width: f64,
        #[serde(rename = "@height")]
        pub height: f64,
    }

    impl ToXml<XmlWriter> for Group {
        fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
            let x = format!("{}", self.x);
            let y = format!("{}", self.y);
            let width = format!("{}", self.width);
            let height = format!("{}", self.height);

            let attrs = vec![
                ("name", self.name.as_str()),
                ("x", x.as_str()),
                ("y", y.as_str()),
                ("width", width.as_str()),
                ("height", height.as_str()),
            ];
            write_tag_with_attrs(writer, "group", "", &attrs)
        }
    }

    impl From<Group> for datamodel::view_element::Group {
        fn from(v: Group) -> Self {
            // XMILE uses top-left coordinates, datamodel uses center
            datamodel::view_element::Group {
                uid: v.uid.unwrap_or(-1),
                name: v.name,
                x: v.x + v.width / 2.0,
                y: v.y + v.height / 2.0,
                width: v.width,
                height: v.height,
            }
        }
    }

    impl From<datamodel::view_element::Group> for Group {
        fn from(v: datamodel::view_element::Group) -> Self {
            // Datamodel uses center coordinates, XMILE uses top-left
            Group {
                name: v.name,
                uid: Some(v.uid),
                x: v.x - v.width / 2.0,
                y: v.y - v.height / 2.0,
                width: v.width,
                height: v.height,
            }
        }
    }

    #[test]
    fn test_group_roundtrip() {
        let cases: &[_] = &[datamodel::view_element::Group {
            uid: 100,
            name: "Economic Sector".to_string(),
            x: 150.0,
            y: 175.0,
            width: 200.0,
            height: 150.0,
        }];
        for expected in cases {
            let expected = expected.clone();
            let actual = datamodel::view_element::Group::from(Group::from(expected.clone()));
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
    Group(view_element::Group),
    // Style(Style),
    #[serde(other)]
    Unhandled,
}

impl ToXml<XmlWriter> for ViewObject {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        match self {
            ViewObject::Aux(aux) => aux.write_xml(writer),
            ViewObject::Stock(stock) => stock.write_xml(writer),
            ViewObject::Flow(flow) => flow.write_xml(writer),
            ViewObject::Link(link) => link.write_xml(writer),
            ViewObject::Module(module) => module.write_xml(writer),
            ViewObject::Cloud(_cloud) => {
                // clouds aren't in the spec, so ignore them here for now
                Ok(())
            }
            ViewObject::Alias(alias) => alias.write_xml(writer),
            ViewObject::Group(group) => group.write_xml(writer),
            ViewObject::Unhandled => {
                // explicitly ignore unhandled things
                Ok(())
            }
        }
    }
}

impl ViewObject {
    pub fn set_uid(&mut self, uid: i32) -> bool {
        match self {
            ViewObject::Aux(aux) => aux.uid = Some(uid),
            ViewObject::Stock(stock) => stock.uid = Some(uid),
            ViewObject::Flow(flow) => flow.uid = Some(uid),
            ViewObject::Link(link) => link.uid = Some(uid),
            ViewObject::Module(module) => module.uid = Some(uid),
            ViewObject::Cloud(cloud) => cloud.uid = uid,
            ViewObject::Alias(alias) => alias.uid = Some(uid),
            ViewObject::Group(group) => group.uid = Some(uid),
            ViewObject::Unhandled => {
                return false;
            }
        };
        true
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
            ViewObject::Group(group) => group.uid,
            ViewObject::Unhandled => None,
        }
    }

    pub fn ident(&self) -> Option<String> {
        match self {
            ViewObject::Aux(aux) => Some(canonicalize(&aux.name).as_str().to_string()),
            ViewObject::Stock(stock) => Some(canonicalize(&stock.name).as_str().to_string()),
            ViewObject::Flow(flow) => Some(canonicalize(&flow.name).as_str().to_string()),
            ViewObject::Link(_link) => None,
            ViewObject::Module(module) => Some(canonicalize(&module.name).as_str().to_string()),
            ViewObject::Cloud(_cloud) => None,
            ViewObject::Alias(_alias) => None,
            // Groups are organizational containers, not model variables
            ViewObject::Group(_group) => None,
            ViewObject::Unhandled => None,
        }
    }

    /// Get the position (x, y) of this ViewObject, if it has one.
    /// Links don't have their own position, so they return None.
    pub fn position(&self) -> Option<(f64, f64)> {
        match self {
            ViewObject::Aux(aux) => Some((aux.x, aux.y)),
            ViewObject::Stock(stock) => Some((stock.x, stock.y)),
            ViewObject::Flow(flow) => Some((flow.x, flow.y)),
            ViewObject::Link(_) => None,
            ViewObject::Module(module) => Some((module.x, module.y)),
            ViewObject::Cloud(cloud) => Some((cloud.x, cloud.y)),
            ViewObject::Alias(alias) => Some((alias.x, alias.y)),
            ViewObject::Group(group) => Some((group.x, group.y)),
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
            ViewObject::Group(v) => {
                datamodel::ViewElement::Group(datamodel::view_element::Group::from(v))
            }
            ViewObject::Unhandled => unreachable!("must filter out unhandled"),
        }
    }
}

impl ViewObject {
    fn from(v: datamodel::ViewElement, view: &datamodel::StockFlow) -> Self {
        match v {
            // TODO: rename ViewObject to ViewElement for consistency
            ViewElement::Aux(v) => ViewObject::Aux(view_element::Aux::from(v)),
            ViewElement::Stock(v) => ViewObject::Stock(view_element::Stock::from(v)),
            ViewElement::Flow(v) => ViewObject::Flow(view_element::Flow::from(v)),
            ViewElement::Link(v) => ViewObject::Link(view_element::Link::from(v, view)),
            ViewElement::Module(v) => ViewObject::Module(view_element::Module::from(v)),
            ViewElement::Alias(v) => ViewObject::Alias(view_element::Alias::from(v, view)),
            ViewElement::Cloud(_v) => ViewObject::Unhandled,
            ViewElement::Group(v) => ViewObject::Group(view_element::Group::from(v)),
        }
    }
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct View {
    #[serde(rename = "@next_uid")]
    pub next_uid: Option<i32>, // used internally
    #[serde(rename = "@type")]
    pub kind: Option<ViewType>,
    #[serde(rename = "@background")]
    pub background: Option<String>,
    #[serde(rename = "@page_width")]
    pub page_width: Option<String>,
    #[serde(rename = "@page_height")]
    pub page_height: Option<String>,
    #[serde(rename = "@show_pages")]
    pub show_pages: Option<bool>,
    #[serde(rename = "$value", default)]
    pub objects: Vec<ViewObject>,
    #[serde(rename = "@zoom")]
    pub zoom: Option<f64>,
    #[serde(rename = "@offset_x")]
    pub offset_x: Option<f64>,
    #[serde(rename = "@offset_y")]
    pub offset_y: Option<f64>,
    #[serde(rename = "@width")]
    pub width: Option<f64>,
    #[serde(rename = "@height")]
    pub height: Option<f64>,
}

impl ToXml<XmlWriter> for View {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        let attrs = &[
            ("isee:show_pages", "false"),
            ("page_width", "800"),
            ("page_height", "600"),
            (
                "view_type",
                self.kind.unwrap_or(ViewType::StockFlow).as_str(),
            ),
        ];
        write_tag_start_with_attrs(writer, "view", attrs)?;

        for element in self.objects.iter() {
            element.write_xml(writer)?;
        }

        write_tag_end(writer, "view")
    }
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
            // don't waste a UID on 'unhandled' objects
            if o.set_uid(next_uid) {
                if let Some(ident) = o.ident() {
                    uid_map.insert(ident, next_uid);
                }
                next_uid += 1;
            }
        }
        for o in self.objects.iter_mut() {
            if let ViewObject::Link(link) = o {
                link.from_uid = match &link.from {
                    LinkEnd::Named(name) => uid_map.get(canonicalize(name).as_str()).cloned(),
                    LinkEnd::Alias(orig_alias) => orig_uid_map.get(&orig_alias.uid).cloned(),
                };
                link.to_uid = match &link.to {
                    LinkEnd::Named(name) => uid_map.get(canonicalize(name).as_str()).cloned(),
                    LinkEnd::Alias(orig_alias) => orig_uid_map.get(&orig_alias.uid).cloned(),
                };
            } else if let ViewObject::Alias(alias) = o {
                let of_ident = canonicalize(&alias.of);
                alias.of_uid = if !of_ident.as_str().is_empty() {
                    uid_map.get(of_ident.as_str()).cloned()
                } else {
                    None
                };
            }
        }

        // if there were links we couldn't resolve, dump them
        self.objects.retain(|o| {
            if let ViewObject::Link(link) = o {
                link.from_uid.is_some() && link.to_uid.is_some()
            } else {
                true
            }
        });

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
            if let Some(Var::Stock(stock)) = model.get_var(&ident) {
                if stock.outflows.is_some() {
                    for outflow in stock.outflows.as_ref().unwrap() {
                        let outflow_ident = canonicalize(outflow).as_str().to_string();
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
                        let inflow_ident = canonicalize(inflow).as_str().to_string();
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
            let source_uid = ends.0.unwrap_or_else(|| {
                let uid = self.next_uid.unwrap();
                self.next_uid = Some(uid + 1);
                let cloud = cloud_for(flow, CloudPosition::Source, uid);
                clouds.push(cloud);
                uid
            });
            let sink_uid = ends.1.unwrap_or_else(|| {
                let uid = self.next_uid.unwrap();
                self.next_uid = Some(uid + 1);
                let cloud = cloud_for(flow, CloudPosition::Sink, uid);
                clouds.push(cloud);
                uid
            });

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
            if let Some(source_uid) = pt1.uid
                && let Some(ViewObject::Stock(stock)) = stocks.get(&source_uid)
            {
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

/// Convert a ViewObject to a datamodel::ViewElement, using the position map for Links.
fn view_object_to_element(
    obj: ViewObject,
    positions: &std::collections::HashMap<i32, (f64, f64)>,
) -> datamodel::ViewElement {
    match obj {
        ViewObject::Aux(v) => datamodel::ViewElement::Aux(datamodel::view_element::Aux::from(v)),
        ViewObject::Stock(v) => {
            datamodel::ViewElement::Stock(datamodel::view_element::Stock::from(v))
        }
        ViewObject::Flow(v) => datamodel::ViewElement::Flow(datamodel::view_element::Flow::from(v)),
        ViewObject::Link(v) => {
            datamodel::ViewElement::Link(view_element::link_from_xmile_with_positions(v, positions))
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
        ViewObject::Group(v) => {
            datamodel::ViewElement::Group(datamodel::view_element::Group::from(v))
        }
        ViewObject::Unhandled => unreachable!("must filter out unhandled"),
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

            // Build a position map from ViewObjects before conversion.
            // This allows Link conversion to detect straight lines based on element positions.
            let positions: std::collections::HashMap<i32, (f64, f64)> = v
                .objects
                .iter()
                .filter_map(|obj| {
                    let uid = obj.uid()?;
                    let pos = obj.position()?;
                    Some((uid, pos))
                })
                .collect();

            datamodel::View::StockFlow(datamodel::StockFlow {
                elements: v
                    .objects
                    .into_iter()
                    .filter(|v| !matches!(v, ViewObject::Unhandled))
                    .map(|obj| view_object_to_element(obj, &positions))
                    .collect(),
                view_box,
                zoom: match v.zoom {
                    None => 1.0,
                    Some(zoom) => {
                        if approx_eq!(f64, zoom, 0.0) {
                            1.0
                        } else {
                            zoom
                        }
                    }
                },
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
                objects: v
                    .elements
                    .iter()
                    .cloned()
                    .map(|element| ViewObject::from(element, &v))
                    .collect(),
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
    use simlin_core::datamodel::Rect;
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
pub struct Module {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@model_name")]
    pub model_name: Option<String>,
    pub doc: Option<String>,
    pub units: Option<String>,
    #[serde(rename = "$value", default)]
    pub refs: Vec<Reference>,
    #[serde(rename = "@access")]
    pub access: Option<String>,
    #[serde(rename = "@ai_state")]
    pub ai_state: Option<String>,
}

fn can_be_module_input(access: &Option<String>) -> bool {
    access
        .as_ref()
        .map(|access| access.eq_ignore_ascii_case("input"))
        .unwrap_or_default()
}

fn visibility(access: &Option<String>) -> Visibility {
    access
        .as_ref()
        .map(|access| {
            if access.eq_ignore_ascii_case("output") {
                Visibility::Public
            } else {
                Visibility::Private
            }
        })
        .unwrap_or(Visibility::Private)
}

fn access_from(visibility: Visibility, can_be_module_input: bool) -> Option<String> {
    if visibility == Visibility::Public {
        Some("output".to_owned())
    } else if can_be_module_input {
        Some("input".to_owned())
    } else {
        None
    }
}

impl ToXml<XmlWriter> for Module {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        let mut attrs = vec![("name", self.name.as_str())];
        if self.model_name.is_some() {
            attrs.push(("simlin:model_name", self.name.as_str()));
        }
        if let Some(access) = self.access.as_ref() {
            attrs.push(("access", access.as_str()));
        }
        write_tag_start_with_attrs(writer, "module", &attrs)?;

        if let Some(ref doc) = self.doc {
            write_tag(writer, "doc", doc)?;
        }
        if let Some(ref units) = self.units {
            write_tag(writer, "units", units)?;
        }

        for reference in self.refs.iter() {
            match reference {
                Reference::Connect(connect) => {
                    let attrs = &[("to", connect.dst.as_str()), ("from", connect.src.as_str())];
                    write_tag_start_with_attrs(writer, "connect", attrs)?;
                    write_tag_end(writer, "connect")?;
                }
                Reference::Connect2(_) => {
                    // explicitly ignore these for now
                }
            }
        }

        if let Some(ref ai_state) = self.ai_state {
            write_tag(writer, "ai_state", ai_state)?;
        }

        write_tag_end(writer, "module")
    }
}

impl From<Module> for datamodel::Module {
    fn from(module: Module) -> Self {
        let ident = module.name.clone();
        // TODO: we should filter these to only module inputs, and rewrite
        //       the equations of variables that use module outputs
        let references: Vec<datamodel::ModuleReference> = module
            .refs
            .into_iter()
            .filter(|r| matches!(r, Reference::Connect(_)))
            .map(|r| {
                if let Reference::Connect(r) = r {
                    datamodel::ModuleReference {
                        src: canonicalize(&r.src).as_str().to_string(),
                        dst: canonicalize(&r.dst).as_str().to_string(),
                    }
                } else {
                    unreachable!();
                }
            })
            .collect();
        datamodel::Module {
            ident,
            model_name: match module.model_name {
                Some(model_name) => canonicalize(&model_name).as_str().to_string(),
                None => canonicalize(&module.name).as_str().to_string(),
            },
            documentation: module.doc.unwrap_or_default(),
            units: module.units,
            references,
            can_be_module_input: can_be_module_input(&module.access),
            visibility: visibility(&module.access),
            ai_state: ai_state_from(module.ai_state),
            uid: None,
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
                    src: canonicalize(&mi.src).to_source_repr(),
                    dst: canonicalize(&mi.dst).to_source_repr(),
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
            access: access_from(module.visibility, module.can_be_module_input),
            ai_state: None, // TODO
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
    #[serde(rename = "@from")]
    pub src: String,
    #[serde(rename = "@to")]
    pub dst: String,
}

#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct NonNegative {}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct VarElement {
    #[serde(rename = "@subscript")]
    pub subscript: String,
    pub eqn: String,
    #[serde(rename = "init_eqn")]
    pub initial_eqn: Option<String>,
    pub gf: Option<Gf>,
}

impl ToXml<XmlWriter> for VarElement {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        let attrs = &[("subscript", self.subscript.as_str())];
        write_tag_start_with_attrs(writer, "element", attrs)?;
        write_tag(writer, "eqn", self.eqn.as_str())?;
        if let Some(init_eqn) = &self.initial_eqn {
            write_tag(writer, "init_eqn", init_eqn.as_str())?;
        }
        if let Some(gf) = &self.gf {
            gf.write_xml(writer)?;
        }
        write_tag_end(writer, "element")
    }
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct Stock {
    #[serde(rename = "@name")]
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
    #[serde(rename = "@access")]
    pub access: Option<String>,
    #[serde(rename = "@ai_state")]
    pub ai_state: Option<String>,
}

impl ToXml<XmlWriter> for Stock {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        let mut attrs = vec![("name", self.name.as_str())];
        if let Some(access) = self.access.as_ref() {
            attrs.push(("access", access.as_str()));
        }
        write_tag_start_with_attrs(writer, "stock", &attrs)?;

        if let Some(VarDimensions {
            dimensions: Some(ref dimensions),
            ..
        }) = self.dimensions
        {
            write_tag_start(writer, "dimensions")?;
            for dim in dimensions.iter() {
                dim.write_xml(writer)?;
            }
            write_tag_end(writer, "dimensions")?;
        }

        if let Some(ref elements) = self.elements {
            for element in elements.iter() {
                element.write_xml(writer)?;
            }
        }

        if let Some(ref eqn) = self.eqn {
            write_tag(writer, "eqn", eqn)?;
        }
        if let Some(ref doc) = self.doc {
            write_tag(writer, "doc", doc)?;
        }
        if let Some(ref units) = self.units {
            write_tag(writer, "units", units)?;
        }

        if let Some(ref inflows) = self.inflows {
            for inflow in inflows.iter() {
                write_tag(writer, "inflow", inflow)?;
            }
        }

        if let Some(ref outflows) = self.outflows {
            for outflow in outflows.iter() {
                write_tag(writer, "outflow", outflow)?;
            }
        }

        if self.non_negative.is_some() {
            write_tag(writer, "non_negative", "")?;
        }

        if let Some(ref ai_state) = self.ai_state {
            write_tag(writer, "ai_state", ai_state)?;
        }

        write_tag_end(writer, "stock")
    }
}

macro_rules! convert_equation(
    ($var:expr) => {{
        if let Some(elements) = $var.elements {
            let dimensions = match $var.dimensions {
                Some(dimensions) => dimensions.dimensions.unwrap().into_iter().map(|e| canonicalize(&e.name).as_str().to_string()).collect(),
                None => vec![],
            };
            let elements = elements.into_iter().map(|e| {
                let canonical_subscripts: Vec<_> = e.subscript.split(",").map(|s| canonicalize(s.trim()).as_str().to_string()).collect();
                (canonical_subscripts.join(","), e.eqn, e.initial_eqn, e.gf.map(datamodel::GraphicalFunction::from))
            }).collect();
            datamodel::Equation::Arrayed(dimensions, elements)
        } else if let Some(dimensions) = $var.dimensions {
            let dimensions = dimensions.dimensions.unwrap_or_default().into_iter().map(|e| canonicalize(&e.name).as_str().to_string()).collect();
            datamodel::Equation::ApplyToAll(dimensions, $var.eqn.unwrap_or_default(), $var.initial_eqn)
        } else {
            datamodel::Equation::Scalar($var.eqn.unwrap_or_default(), $var.initial_eqn)
        }
    }}
);

// TODO: forked the above function because stocks don't have `init_eqn` fields.  Probably a macro-ish way to fix this.
macro_rules! convert_stock_equation(
    ($var:expr) => {{
        if let Some(elements) = $var.elements {
            let dimensions = match $var.dimensions {
                Some(dimensions) => dimensions.dimensions.unwrap().into_iter().map(|e| canonicalize(&e.name).as_str().to_string()).collect(),
                None => vec![],
            };
            let elements = elements.into_iter().map(|e| {
                let canonical_subscripts: Vec<_> = e.subscript.split(",").map(|s| canonicalize(s.trim()).as_str().to_string()).collect();
                (canonical_subscripts.join(","), e.eqn, e.initial_eqn, e.gf.map(datamodel::GraphicalFunction::from))
            }).collect();
            datamodel::Equation::Arrayed(dimensions, elements)
        } else if let Some(dimensions) = $var.dimensions {
            let dimensions = dimensions.dimensions.unwrap_or_default().into_iter().map(|e| canonicalize(&e.name).as_str().to_string()).collect();
            datamodel::Equation::ApplyToAll(dimensions, $var.eqn.unwrap_or_default(), None)
        } else {
            datamodel::Equation::Scalar($var.eqn.unwrap_or_default(), None)
        }
    }}
);

fn ai_state_from(s: Option<String>) -> Option<datamodel::AiState> {
    s.map(|s| {
        use datamodel::AiState::*;
        match s.to_lowercase().as_str() {
            "a" => A,
            "b" => B,
            "c" => C,
            "d" => D,
            "e" => E,
            "f" => F,
            "g" => G,
            "h" => H,
            _ => A,
        }
    })
}

impl From<Stock> for datamodel::Stock {
    fn from(stock: Stock) -> Self {
        let inflows = stock
            .inflows
            .unwrap_or_default()
            .into_iter()
            .map(|id| canonicalize(&id).as_str().to_string())
            .collect();
        let outflows = stock
            .outflows
            .unwrap_or_default()
            .into_iter()
            .map(|id| canonicalize(&id).as_str().to_string())
            .collect();
        datamodel::Stock {
            ident: stock.name.clone(),
            equation: convert_stock_equation!(stock),
            documentation: stock.doc.unwrap_or_default(),
            units: stock.units,
            inflows,
            outflows,
            non_negative: stock.non_negative.is_some(),
            can_be_module_input: can_be_module_input(&stock.access),
            visibility: visibility(&stock.access),
            ai_state: ai_state_from(stock.ai_state),
            uid: None,
        }
    }
}

impl From<datamodel::Stock> for Stock {
    fn from(stock: datamodel::Stock) -> Self {
        Stock {
            name: stock.ident,
            eqn: match &stock.equation {
                Equation::Scalar(eqn, ..) => {
                    if eqn.is_empty() {
                        None
                    } else {
                        Some(eqn.clone())
                    }
                }
                Equation::ApplyToAll(_, eqn, ..) => {
                    if eqn.is_empty() {
                        None
                    } else {
                        Some(eqn.clone())
                    }
                }
                Equation::Arrayed(..) => None,
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
                Equation::Scalar(..) => None,
                Equation::ApplyToAll(dims, ..) => Some(VarDimensions {
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
                Equation::Scalar(..) => None,
                Equation::ApplyToAll(..) => None,
                Equation::Arrayed(_, elements) => Some(
                    elements
                        .into_iter()
                        .map(|(subscript, eqn, _, gf)| VarElement {
                            subscript,
                            eqn,
                            initial_eqn: None,
                            gf: gf.map(Gf::from),
                        })
                        .collect(),
                ),
            },
            access: access_from(stock.visibility, stock.can_be_module_input),
            ai_state: None, // TODO
        }
    }
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct Flow {
    #[serde(rename = "@name")]
    pub name: String,
    pub eqn: Option<String>,
    #[serde(rename = "init_eqn")]
    pub initial_eqn: Option<String>,
    pub doc: Option<String>,
    pub units: Option<String>,
    pub gf: Option<Gf>,
    pub non_negative: Option<NonNegative>,
    pub dimensions: Option<VarDimensions>,
    #[serde(rename = "element", default)]
    pub elements: Option<Vec<VarElement>>,
    #[serde(rename = "@access")]
    pub access: Option<String>,
    #[serde(rename = "@ai_state")]
    pub ai_state: Option<String>,
}

impl ToXml<XmlWriter> for Flow {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        let mut attrs = vec![("name", self.name.as_str())];
        if let Some(access) = self.access.as_ref() {
            attrs.push(("access", access.as_str()));
        }
        write_tag_start_with_attrs(writer, "flow", &attrs)?;

        if let Some(VarDimensions {
            dimensions: Some(ref dimensions),
            ..
        }) = self.dimensions
        {
            write_tag_start(writer, "dimensions")?;
            for dim in dimensions.iter() {
                dim.write_xml(writer)?;
            }
            write_tag_end(writer, "dimensions")?;
        }

        if let Some(ref elements) = self.elements {
            for element in elements.iter() {
                element.write_xml(writer)?;
            }
        }

        if let Some(ref eqn) = self.eqn {
            write_tag(writer, "eqn", eqn)?;
        }
        if let Some(ref eqn) = self.initial_eqn {
            write_tag(writer, "init_eqn", eqn)?;
        }
        if let Some(ref doc) = self.doc {
            write_tag(writer, "doc", doc)?;
        }
        if let Some(ref units) = self.units {
            write_tag(writer, "units", units)?;
        }
        if let Some(ref gf) = self.gf {
            gf.write_xml(writer)?;
        }

        if self.non_negative.is_some() {
            write_tag(writer, "non_negative", "")?;
        }

        if let Some(ref ai_state) = self.ai_state {
            write_tag(writer, "ai_state", ai_state)?;
        }

        write_tag_end(writer, "flow")
    }
}

impl From<Flow> for datamodel::Flow {
    fn from(flow: Flow) -> Self {
        datamodel::Flow {
            ident: flow.name.clone(),
            equation: convert_equation!(flow),
            documentation: flow.doc.unwrap_or_default(),
            units: flow.units,
            gf: flow.gf.map(datamodel::GraphicalFunction::from),
            non_negative: flow.non_negative.is_some(),
            can_be_module_input: can_be_module_input(&flow.access),
            visibility: visibility(&flow.access),
            ai_state: ai_state_from(flow.ai_state),
            uid: None,
        }
    }
}

impl From<datamodel::Flow> for Flow {
    fn from(flow: datamodel::Flow) -> Self {
        Flow {
            name: flow.ident,
            eqn: match &flow.equation {
                Equation::Scalar(eqn, ..) => {
                    if eqn.is_empty() {
                        None
                    } else {
                        Some(eqn.clone())
                    }
                }
                Equation::ApplyToAll(_, eqn, ..) => {
                    if eqn.is_empty() {
                        None
                    } else {
                        Some(eqn.clone())
                    }
                }
                Equation::Arrayed(_, _) => None,
            },
            initial_eqn: match &flow.equation {
                Equation::Scalar(.., initial_eqn) => initial_eqn.clone(),
                Equation::ApplyToAll(.., initial_eqn) => initial_eqn.clone(),
                Equation::Arrayed(..) => None,
            },
            doc: if flow.documentation.is_empty() {
                None
            } else {
                Some(flow.documentation)
            },
            units: flow.units,
            gf: flow.gf.map(Gf::from),
            non_negative: if flow.non_negative {
                Some(NonNegative {})
            } else {
                None
            },
            dimensions: match &flow.equation {
                Equation::Scalar(..) => None,
                Equation::ApplyToAll(dims, ..) => Some(VarDimensions {
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
                Equation::Scalar(..) => None,
                Equation::ApplyToAll(..) => None,
                Equation::Arrayed(_, elements) => Some(
                    elements
                        .into_iter()
                        .map(|(subscript, eqn, initial_eqn, gf)| VarElement {
                            subscript,
                            eqn,
                            initial_eqn,
                            gf: gf.map(Gf::from),
                        })
                        .collect(),
                ),
            },
            access: access_from(flow.visibility, flow.can_be_module_input),
            ai_state: None, // TODO
        }
    }
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct Aux {
    #[serde(rename = "@name")]
    pub name: String,
    pub eqn: Option<String>,
    #[serde(rename = "init_eqn")]
    pub initial_eqn: Option<String>,
    pub doc: Option<String>,
    pub units: Option<String>,
    pub gf: Option<Gf>,
    pub dimensions: Option<VarDimensions>,
    #[serde(rename = "element", default)]
    pub elements: Option<Vec<VarElement>>,
    #[serde(rename = "@access")]
    pub access: Option<String>,
    #[serde(rename = "@ai_state")]
    pub ai_state: Option<String>,
}

impl ToXml<XmlWriter> for Aux {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        let mut attrs = vec![("name", self.name.as_str())];
        if let Some(access) = self.access.as_ref() {
            attrs.push(("access", access.as_str()));
        }
        write_tag_start_with_attrs(writer, "aux", &attrs)?;

        if let Some(VarDimensions {
            dimensions: Some(ref dimensions),
            ..
        }) = self.dimensions
        {
            write_tag_start(writer, "dimensions")?;
            for dim in dimensions.iter() {
                dim.write_xml(writer)?;
            }
            write_tag_end(writer, "dimensions")?;
        }

        if let Some(ref elements) = self.elements {
            for element in elements.iter() {
                element.write_xml(writer)?;
            }
        }

        if let Some(ref eqn) = self.eqn {
            write_tag(writer, "eqn", eqn)?;
        }
        if let Some(ref eqn) = self.initial_eqn {
            write_tag(writer, "init_eqn", eqn)?;
        }
        if let Some(ref doc) = self.doc {
            write_tag(writer, "doc", doc)?;
        }
        if let Some(ref units) = self.units {
            write_tag(writer, "units", units)?;
        }

        if let Some(ref gf) = self.gf {
            gf.write_xml(writer)?;
        }

        if let Some(ref ai_state) = self.ai_state {
            write_tag(writer, "ai_state", ai_state)?;
        }

        write_tag_end(writer, "aux")
    }
}

impl From<Aux> for datamodel::Aux {
    fn from(aux: Aux) -> Self {
        datamodel::Aux {
            ident: aux.name.clone(),
            equation: convert_equation!(aux),
            documentation: aux.doc.unwrap_or_default(),
            units: aux.units,
            gf: aux.gf.map(datamodel::GraphicalFunction::from),
            can_be_module_input: can_be_module_input(&aux.access),
            visibility: visibility(&aux.access),
            ai_state: ai_state_from(aux.ai_state),
            uid: None,
        }
    }
}

impl From<datamodel::Aux> for Aux {
    fn from(aux: datamodel::Aux) -> Self {
        Aux {
            name: aux.ident,
            eqn: match &aux.equation {
                Equation::Scalar(eqn, ..) => {
                    if eqn.is_empty() {
                        None
                    } else {
                        Some(eqn.clone())
                    }
                }
                Equation::ApplyToAll(_, eqn, ..) => {
                    if eqn.is_empty() {
                        None
                    } else {
                        Some(eqn.clone())
                    }
                }
                Equation::Arrayed(_, _) => None,
            },
            initial_eqn: match &aux.equation {
                Equation::Scalar(.., initial_eqn) => initial_eqn.clone(),
                Equation::ApplyToAll(.., initial_eqn) => initial_eqn.clone(),
                Equation::Arrayed(..) => None,
            },
            doc: if aux.documentation.is_empty() {
                None
            } else {
                Some(aux.documentation)
            },
            units: aux.units,
            gf: aux.gf.map(Gf::from),
            dimensions: match &aux.equation {
                Equation::Scalar(..) => None,
                Equation::ApplyToAll(dims, ..) => Some(VarDimensions {
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
                Equation::Scalar(..) => None,
                Equation::ApplyToAll(..) => None,
                Equation::Arrayed(_, elements) => Some(
                    elements
                        .into_iter()
                        .map(|(subscript, eqn, initial_eqn, gf)| VarElement {
                            subscript,
                            eqn,
                            initial_eqn,
                            gf: gf.map(Gf::from),
                        })
                        .collect(),
                ),
            },
            access: access_from(aux.visibility, aux.can_be_module_input),
            ai_state: None, // TODO
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

impl ToXml<XmlWriter> for Var {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        match self {
            Var::Stock(stock) => stock.write_xml(writer),
            Var::Flow(flow) => flow.write_xml(writer),
            Var::Aux(aux) => aux.write_xml(writer),
            Var::Module(module) => module.write_xml(writer),
            Var::Unhandled => Ok(()),
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
    let input = Var::Stock(Stock {
        name: "Heat Loss To Room".to_string(),
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
        access: None,
        ai_state: None,
    });

    let expected = datamodel::Variable::Stock(datamodel::Stock {
        ident: "Heat Loss To Room".to_string(),
        equation: Equation::Scalar("total_population".to_string(), None),
        documentation: "People who can contract the disease.".to_string(),
        units: Some("people".to_string()),
        inflows: vec!["solar_radiation".to_string()],
        outflows: vec!["succumbing".to_string(), "succumbing_2".to_string()],
        non_negative: false,
        can_be_module_input: false,
        visibility: Visibility::Private,
        ai_state: None,
        uid: None,
    });

    let output = datamodel::Variable::from(input);

    assert_eq!(expected, output);
}

pub fn project_to_xmile(project: &datamodel::Project) -> Result<String> {
    let file: File = project.clone().into();

    let mut writer = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 4);

    writer
        .write_event(Event::Decl(BytesDecl::new("1.0", Some("utf-8"), None)))
        .unwrap();
    file.write_xml(&mut writer)?;

    let result = writer.into_inner().into_inner();

    use simlin_core::common::{Error, ErrorCode, ErrorKind};
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

    Ok(convert_file_to_project(file))
}

pub fn convert_file_to_project(file: File) -> datamodel::Project {
    datamodel::Project::from(file)
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
        access: None,
        ai_state: None,
    };

    use quick_xml::de;
    let stock: Var = de::from_reader(input.as_bytes()).unwrap();

    if let Var::Stock(stock) = stock {
        assert_eq!(expected, stock);
    } else {
        panic!("not a stock");
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
        initial_eqn: None,
        doc: None,
        units: None,
        gf: None,
        dimensions: None,
        elements: None,
        access: None,
        ai_state: None,
    };

    use quick_xml::de;
    let aux: Var = de::from_reader(input.as_bytes()).unwrap();

    if let Var::Aux(aux) = aux {
        assert_eq!(expected, aux);
    } else {
        panic!("not an aux");
    }
}

#[test]
fn test_xml_gf_parsing() {
    let input = "            <aux name=\"lookup function table\" access=\"input\">
                <eqn>0</eqn>
                <init_eqn>55</init_eqn>
                <gf>
                    <yscale min=\"-1\" max=\"1\"/>
                    <xpts>0,5,10,15,20,25,30,35,40,45</xpts>
                    <ypts>0,0,1,1,0,0,-1,-1,0,0</ypts>
                </gf>
            </aux>";

    let expected = Aux {
        name: "lookup function table".to_string(),
        eqn: Some("0".to_string()),
        initial_eqn: Some("55".to_string()),
        doc: None,
        units: None,
        gf: Some(Gf {
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
        access: Some("input".to_owned()),
        ai_state: None,
    };

    use quick_xml::de;
    let aux: Var = de::from_reader(input.as_bytes()).unwrap();

    if let Var::Aux(aux) = aux {
        assert_eq!(expected, aux);
    } else {
        panic!("not an aux");
    }
}

#[test]
fn test_module_parsing() {
    let input = "<module name=\"hares\" simlin:model_name=\"hares3\" access=\"output\">
				<connect to=\"hares.area\" from=\".area\"/>
				<connect2 to=\"hares.area\" from=\"area\"/>
				<connect to=\"lynxes.hare_density\" from=\"hares.hare_density\"/>
				<connect2 to=\"lynxes.hare_density\" from=\"hares.hare_density\"/>
				<connect to=\"hares.lynxes\" from=\"lynxes.lynxes\"/>
				<connect2 to=\"hares.lynxes\" from=\"lynxes.lynxes\"/>
			</module>";

    let expected = Module {
        name: "hares".to_string(),
        model_name: Some("hares3".to_owned()),
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
        access: Some("output".to_owned()),
        ai_state: None,
    };

    use quick_xml::de;
    let actual: Module = de::from_reader(input.as_bytes()).unwrap();
    assert_eq!(expected, actual);

    let expected_roundtripped = Module {
        name: "hares".to_string(),
        model_name: Some("hares3".to_string()),
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
        access: Some("output".to_owned()),
        ai_state: None,
    };

    let roundtripped = Module::from(datamodel::Module::from(actual));
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

#[test]
fn test_per_element_gf_parsing() {
    let input = r#"<aux name="c">
        <element subscript="A1">
            <eqn>0</eqn>
            <gf>
                <xpts>0,1</xpts>
                <ypts>10,20</ypts>
            </gf>
        </element>
        <element subscript="A2">
            <eqn>0</eqn>
            <gf>
                <xpts>0,1</xpts>
                <ypts>20,30</ypts>
            </gf>
        </element>
        <dimensions>
            <dim name="DimA"/>
        </dimensions>
    </aux>"#;

    use quick_xml::de;
    let aux: Var = de::from_reader(input.as_bytes()).unwrap();

    if let Var::Aux(aux) = aux {
        let elements = aux.elements.as_ref().expect("elements should exist");
        assert_eq!(2, elements.len());

        // Check that per-element gf is parsed
        let elem_a1 = &elements[0];
        assert_eq!("A1", elem_a1.subscript);
        let gf_a1 = elem_a1.gf.as_ref().expect("A1 should have gf");
        assert_eq!(Some("0,1".to_string()), gf_a1.x_pts);
        assert_eq!(Some("10,20".to_string()), gf_a1.y_pts);

        let elem_a2 = &elements[1];
        assert_eq!("A2", elem_a2.subscript);
        let gf_a2 = elem_a2.gf.as_ref().expect("A2 should have gf");
        assert_eq!(Some("0,1".to_string()), gf_a2.x_pts);
        assert_eq!(Some("20,30".to_string()), gf_a2.y_pts);
    } else {
        panic!("not an aux");
    }
}

#[test]
fn test_dimension_with_maps_to_parsing() {
    // Test deserialization of dimension with maps_to element
    let input = r#"<dim name="DimA">
            <elem name="A1"/>
            <elem name="A2"/>
            <elem name="A3"/>
            <isee:maps_to>DimB</isee:maps_to>
        </dim>"#;

    let expected = Dimension {
        name: "DimA".to_string(),
        size: None,
        elements: Some(vec![
            Index {
                name: "A1".to_string(),
            },
            Index {
                name: "A2".to_string(),
            },
            Index {
                name: "A3".to_string(),
            },
        ]),
        maps_to: Some("DimB".to_string()),
    };

    use quick_xml::de;
    let actual: Dimension = de::from_reader(input.as_bytes()).unwrap();
    assert_eq!(expected, actual);
}
