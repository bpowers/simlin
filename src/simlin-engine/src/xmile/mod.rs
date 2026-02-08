// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::io::{BufRead, Cursor, Write};

use crate::common::Result;
use crate::datamodel;
use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

pub mod dimensions;
pub mod model;
pub mod variables;
pub mod views;

// Re-export submodule types that form the public API
pub use self::dimensions::{Dimension, Gf, GraphicalFunctionKind, GraphicalFunctionScale, Index};
pub use self::model::{
    Connect, Model, Module, NonNegative, Reference, SemanticGroup, Variables, Views,
};
pub use self::variables::{Aux, Flow, Stock, Var, VarElement};
pub use self::views::view_element;
pub use self::views::{View, ViewObject, ViewType};

pub(crate) trait ToXml<W: Clone + Write> {
    fn write_xml(&self, writer: &mut Writer<W>) -> Result<()>;
}

pub(crate) type XmlWriter = Cursor<Vec<u8>>;

pub(crate) const STOCK_WIDTH: f64 = 45.0;
pub(crate) const STOCK_HEIGHT: f64 = 35.0;

macro_rules! import_err(
    ($code:tt, $str:expr) => {{
        use crate::common::{Error, ErrorCode, ErrorKind};
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct AiInformation {
    pub status: AiStatus,
    pub testing: Option<AiTesting>,
    pub log: Option<String>,
    // TODO: settings
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Serialize)]
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct AiTesting {
    #[serde(rename = "@signed_message_body")]
    pub signed_message_body: String,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Data {
    // TODO
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Macro {
    // TODO
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct VarDimensions {
    #[serde(rename = "dim")]
    pub dimensions: Option<Vec<VarDimension>>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Dimensions {
    #[serde(rename = "dim")]
    pub dimensions: Option<Vec<Dimension>>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
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
    pub created: Option<String>, // ISO 8601 date format, e.g. " 2014-08-10"
    pub modified: Option<String>, // ISO 8601 date format
    pub uuid: Option<String>,    // IETF RFC4122 format (84-4-4-12 hex digits with the dashes)
    pub includes: Option<Includes>,
}

pub(crate) fn xml_error(err: std::io::Error) -> crate::common::Error {
    use crate::common::{Error, ErrorCode, ErrorKind};

    Error::new(
        ErrorKind::Import,
        ErrorCode::XmlDeserialization,
        Some(err.to_string()),
    )
}

pub(crate) fn write_tag_start(writer: &mut Writer<XmlWriter>, tag_name: &str) -> Result<()> {
    write_tag_start_with_attrs(writer, tag_name, &[])
}

pub(crate) fn write_tag_start_with_attrs(
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

pub(crate) fn write_tag_end(writer: &mut Writer<XmlWriter>, tag_name: &str) -> Result<()> {
    writer
        .write_event(Event::End(BytesEnd::new(tag_name)))
        .map_err(xml_error)
}

pub(crate) fn write_tag_text(writer: &mut Writer<XmlWriter>, content: &str) -> Result<()> {
    writer
        .write_event(Event::Text(BytesText::new(content)))
        .map_err(xml_error)
}

pub(crate) fn write_tag(
    writer: &mut Writer<XmlWriter>,
    tag_name: &str,
    content: &str,
) -> Result<()> {
    write_tag_with_attrs(writer, tag_name, content, &[])
}

pub(crate) fn write_tag_with_attrs(
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Caption {}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Includes {}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Image {
    #[serde(default)]
    pub resource: String, // "JPG, GIF, TIF, or PNG" path, URL, or image embedded in base64 data URI
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Product {
    #[serde(rename = "$value")]
    pub name: Option<String>,
    #[serde(rename = "lang")]
    pub language: Option<String>,
    pub version: Option<String>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize, Hash)]
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Options {
    pub namespace: Option<String>, // string of comma separated namespaces
    #[serde(rename = "$value")]
    pub features: Option<Vec<Feature>>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
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
            sim_method: match sim_method.to_lowercase().as_str() {
                "euler" => datamodel::SimMethod::Euler,
                "rk2" => datamodel::SimMethod::RungeKutta2,
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
                datamodel::SimMethod::RungeKutta2 => "rk2".to_string(),
                datamodel::SimMethod::RungeKutta4 => "rk4".to_string(),
            }),
            time_units: sim_specs.time_units,
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Behavior {
    // TODO
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Style {
    // TODO
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Units {
    pub unit: Option<Vec<Unit>>,
}
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
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

pub fn project_to_xmile(project: &datamodel::Project) -> Result<String> {
    let file: File = project.clone().into();

    let mut writer = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 4);

    writer
        .write_event(Event::Decl(BytesDecl::new("1.0", Some("utf-8"), None)))
        .unwrap();
    file.write_xml(&mut writer)?;

    let result = writer.into_inner().into_inner();

    use crate::common::{Error, ErrorCode, ErrorKind};
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
