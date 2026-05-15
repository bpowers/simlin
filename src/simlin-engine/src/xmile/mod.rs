// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::io::{BufRead, Cursor, Write};

use crate::common::{Result, canonicalize};
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
    XmileLoopMetadata,
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

        // A <macro> is a top-level sibling of <model>; emit them after the
        // models, before closing </xmile>.
        for mac in self.macros.iter() {
            mac.write_xml(writer)?;
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
        // Partition models: macro-marked models become top-level <macro>
        // siblings of <model>; the rest stay as ordinary <model>s.
        let (macro_models, plain_models): (Vec<_>, Vec<_>) = project
            .models
            .into_iter()
            .partition(|m| m.macro_spec.is_some());

        // The project's macro registry (canonical macro-model name -> its
        // MacroSpec). Drives the multi-output-invocation extraction so a
        // materialized Variable::Module + binding auxes round-trips through
        // a `simlin:` extension instead of a non-standard <module>.
        let macro_specs: HashMap<String, datamodel::MacroSpec> = macro_models
            .iter()
            .filter_map(|m| {
                m.macro_spec
                    .clone()
                    .map(|spec| (canonicalize(&m.name).into_owned(), spec))
            })
            .collect();

        let macros: Vec<Macro> = macro_models.into_iter().map(Macro::from).collect();

        // The <uses_macros> header option is emitted whenever the project
        // contains at least one macro. Simlin does not support recursive
        // macros and emits both attributes as fixed "false" -- a
        // deterministic emission that keeps the byte-stable round-trip
        // stable.
        let options = if macros.is_empty() {
            None
        } else {
            Some(Options {
                namespace: None,
                features: Some(vec![Feature::UsesMacros {
                    recursive_macros: Some(false),
                    option_filters: Some(false),
                }]),
            })
        };

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
                options,
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
            models: plain_models
                .into_iter()
                .map(|m| {
                    // Extract materialized multi-output invocation clusters
                    // into `simlin:` extension records; the residual model
                    // (cluster vars removed) converts via the ordinary
                    // per-model bridge. Both empty when the project has no
                    // multi-output macros, so non-macro projects are
                    // unaffected.
                    let (residual, invocations) = model::extract_macro_invocations(m, &macro_specs);
                    let mut xmile_model = Model::from(residual);
                    if !invocations.is_empty() {
                        xmile_model.macro_invocations = Some(invocations);
                    }
                    xmile_model
                })
                .collect(),
            macros,
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

/// A formal parameter of a `<macro>`: the parameter name is the element
/// text; an optional `default` attribute supplies a default value.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Parm {
    #[serde(rename = "$text")]
    pub name: String,
    #[serde(rename = "@default", skip_serializing_if = "Option::is_none", default)]
    pub default: Option<String>,
}

/// A top-level XMILE `<macro>` element (a sibling of `<model>`). See
/// `docs/reference/xmile-v1.0.html` §4.8. The XMILE handling is asymmetric:
/// this type is deserialized via serde derives and serialized via a
/// hand-written `ToXml` impl (Task 2). `Eq` is deliberately *not* derived --
/// the `variables`/`sim_specs` fields transitively contain `f64`.
///
/// Deliberately *no* `views` field: the xmutil-emitted `<macro>` carries a
/// `<views>` child (a macro-body diagram) but macro models are non-navigable,
/// so a macro body's views are inert. `quick_xml::de` silently ignores the
/// unknown `<views>` on read and the writer never emits one (a documented
/// intentional non-round-trip).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct Macro {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "parm", default)]
    pub parms: Vec<Parm>,
    /// The expression-form body / primary-output expression (`<eqn>`).
    pub eqn: Option<String>,
    /// The multi-equation body (§4.8.2); reuses the `<model>` content model.
    pub variables: Option<Variables>,
    /// Present only for parse-completeness; a non-empty value is the
    /// documented unsupported limitation and is rejected at conversion.
    pub sim_specs: Option<SimSpecs>,
    pub doc: Option<String>,
    #[serde(rename = "@namespace")]
    pub namespace: Option<String>,
    /// `simlin:`-namespaced extension: additional output port names, in
    /// order, for a Vensim multi-output (`:`-list) macro. Empty for an
    /// ordinary single-output macro. quick-xml strips the `simlin:` prefix on
    /// read, so the serde rename is the namespace-stripped local name.
    /// Added in Task 3; `#[serde(default)]` so the Task 1 reader compiles.
    #[serde(rename = "additional-outputs", default)]
    pub additional_outputs: Option<MacroAdditionalOutputs>,
}

/// `simlin:`-namespaced extension element recording a multi-output macro's
/// additional output ports. Serialized as
/// `<simlin:additional-outputs names="minval,maxval"/>` -- a comma-separated
/// `names` attribute in declaration order (mirrors `XmileLoopMetadata`'s
/// comma-joined `uids_text`). Emitted only when a macro has additional
/// outputs (single-output macros stay standards-clean -- AC4.5).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct MacroAdditionalOutputs {
    #[serde(rename = "@names", default)]
    pub names: String,
}

impl ToXml<XmlWriter> for MacroAdditionalOutputs {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        let attrs = &[("names", self.names.as_str())];
        write_tag_empty_with_attrs(writer, "simlin:additional-outputs", attrs)
    }
}

impl ToXml<XmlWriter> for Macro {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        // Hand-written, mirroring Model::write_xml. Element order follows the
        // XMILE spec / xmutil shape: <eqn>, then <parm>s, then <variables>,
        // then <doc>, then the simlin: additional-outputs extension.
        let mut attrs = vec![("name", self.name.as_str())];
        if let Some(ref ns) = self.namespace {
            attrs.push(("namespace", ns.as_str()));
        }
        write_tag_start_with_attrs(writer, "macro", &attrs)?;

        if let Some(ref eqn) = self.eqn {
            write_tag(writer, "eqn", eqn)?;
        }

        for parm in self.parms.iter() {
            if let Some(ref default) = parm.default {
                let parm_attrs = &[("default", default.as_str())];
                write_tag_with_attrs(writer, "parm", parm.name.as_str(), parm_attrs)?;
            } else {
                write_tag(writer, "parm", parm.name.as_str())?;
            }
        }

        write_tag_start(writer, "variables")?;
        if let Some(Variables { ref variables }) = self.variables {
            for var in variables.iter() {
                var.write_xml(writer)?;
            }
        }
        write_tag_end(writer, "variables")?;

        if let Some(ref doc) = self.doc {
            write_tag(writer, "doc", doc)?;
        }

        // simlin: additional-outputs extension (Task 3): present only for a
        // multi-output macro. A single-output macro leaves this `None`, so a
        // single-output-only project stays standards-clean (AC4.5).
        if let Some(ref ao) = self.additional_outputs {
            ao.write_xml(writer)?;
        }

        write_tag_end(writer, "macro")
    }
}

impl From<datamodel::Model> for Macro {
    /// Convert a macro-marked `datamodel::Model` into an `xmile::Macro`.
    ///
    /// The synthesized formal-parameter port variables are excluded from
    /// `<variables>`: they are reconstructed from the `<parm>`s by
    /// `Model::new_macro` on re-import, so emitting them in `<variables>` too
    /// would be redundant and would break round-trip stability.
    fn from(model: datamodel::Model) -> Self {
        // A macro model always has a MacroSpec (the partition in
        // `From<datamodel::Project> for File` only routes macro-marked models
        // here). Defaulting keeps this total without an unwrap.
        let spec = model.macro_spec.clone().unwrap_or(datamodel::MacroSpec {
            parameters: vec![],
            primary_output: model.name.clone(),
            additional_outputs: vec![],
        });

        let parm_set: std::collections::HashSet<String> = spec
            .parameters
            .iter()
            .map(|p| canonicalize(p).into_owned())
            .collect();

        // Body variables minus the synthesized parameter ports.
        let body: Vec<Var> = model
            .variables
            .into_iter()
            .filter(|v| !parm_set.contains(canonicalize(v.get_ident()).as_ref()))
            .map(Var::from)
            .collect();

        let parms: Vec<Parm> = spec
            .parameters
            .iter()
            .map(|p| Parm {
                name: p.clone(),
                default: None,
            })
            .collect();

        // The simlin: additional-outputs extension is emitted ONLY for a
        // multi-output macro (non-empty additional_outputs); a single-output
        // macro stays standards-clean (AC4.5).
        let additional_outputs = if spec.additional_outputs.is_empty() {
            None
        } else {
            Some(MacroAdditionalOutputs {
                names: spec.additional_outputs.join(","),
            })
        };

        Macro {
            name: model.name,
            parms,
            // <eqn> holds the primary-output name (the xmutil shape: the
            // body's primary-output equation is named after the macro).
            eqn: Some(spec.primary_output),
            variables: if body.is_empty() {
                None
            } else {
                Some(Variables { variables: body })
            },
            // Per-macro <sim_specs> is the documented unsupported limitation;
            // never emitted.
            sim_specs: None,
            doc: None,
            namespace: None,
            additional_outputs,
        }
    }
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

/// Emit a self-closing element (`<tag .../>`). Unlike
/// `write_tag_with_attrs(.., "", ..)` (which renders `<tag ..></tag>` under
/// the indenting writer), this produces the compact self-closing form the
/// XMILE `<uses_macros>` / `<simlin:additional-outputs>` extensions use.
pub(crate) fn write_tag_empty_with_attrs(
    writer: &mut Writer<XmlWriter>,
    tag_name: &str,
    attrs: &[(&str, &str)],
) -> Result<()> {
    let mut elem = BytesStart::new(tag_name);
    for attr in attrs.iter() {
        elem.push_attribute(*attr);
    }
    writer.write_event(Event::Empty(elem)).map_err(xml_error)
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

        // options / features. Today the only feature the writer emits is
        // <uses_macros> (when the project contains a macro); the attributes
        // are deterministic fixed "false" (Simlin supports neither recursive
        // macros nor option filters), keeping the byte-stable round-trip
        // stable.
        if let Some(Options {
            features: Some(ref features),
            ..
        }) = self.options
            && !features.is_empty()
        {
            write_tag_start(writer, "options")?;
            for feature in features.iter() {
                feature.write_xml(writer)?;
            }
            write_tag_end(writer, "options")?;
        }

        write_tag_end(writer, "header")
    }
}

impl ToXml<XmlWriter> for Feature {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        match self {
            Feature::UsesMacros {
                recursive_macros,
                option_filters,
            } => {
                let recursive = if recursive_macros.unwrap_or(false) {
                    "true"
                } else {
                    "false"
                };
                let opt_filters = if option_filters.unwrap_or(false) {
                    "true"
                } else {
                    "false"
                };
                let attrs = &[
                    ("recursive_macros", recursive),
                    ("option_filters", opt_filters),
                ];
                write_tag_empty_with_attrs(writer, "uses_macros", attrs)
            }
            // The other features are not emitted by the writer today; the
            // round-trip never produces them, so this is unreachable in
            // practice. Emit nothing rather than panic.
            _ => Ok(()),
        }
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
        // Spec-required attributes (XMILE §2.2). Without the `@` rename
        // serde would (de)serialize these as child elements, so the reader
        // and the Task 2 writer (which emits the spec-correct attribute
        // form) would disagree and the round-trip would be lossy.
        #[serde(rename = "@recursive_macros")]
        recursive_macros: Option<bool>,
        #[serde(rename = "@option_filters")]
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

    convert_file_to_project(file)
}

/// Convert a parsed XMILE `File` into a `datamodel::Project`.
///
/// Fallible because a `<macro>` with a non-empty per-macro `<sim_specs>` is
/// the documented unsupported limitation (a macro running with its own
/// dt/stop time) and is rejected here with a clear error.
pub fn convert_file_to_project(file: File) -> Result<datamodel::Project> {
    let mut project = datamodel::Project::from(file.clone());

    // A macro is a top-level `<macro>` element, a sibling of `<model>`. The
    // bridge between `file.macros` and the macro-marked entries of
    // `project.models` lives here at the File <-> Project level (the macro
    // *body* reuses the per-`Model` `xmile::Variables` conversion).
    for mac in file.macros {
        project.models.push(macro_to_datamodel(mac)?);
    }

    Ok(project)
}

/// Canonicalize a macro-body variable's structural idents (its own ident and,
/// for a stock, its `inflows`/`outflows` name lists) to the engine ident
/// form. This makes the body's stored idents byte-identical to the canonical
/// `MacroSpec` names and to `Model::new_macro`'s parameter-port matching,
/// mirroring the MDL path where body variables are produced in
/// `variable_ident` form.
fn canonicalize_body_variable_idents(var: datamodel::Variable) -> datamodel::Variable {
    let mut var = var;
    let canon = canonicalize(var.get_ident()).into_owned();
    var.set_ident(canon);
    if let datamodel::Variable::Stock(stock) = &mut var {
        for f in stock.inflows.iter_mut() {
            *f = canonicalize(f).into_owned();
        }
        for f in stock.outflows.iter_mut() {
            *f = canonicalize(f).into_owned();
        }
    }
    var
}

/// Convert one `xmile::Macro` into a macro-marked `datamodel::Model`.
///
/// The XMILE reader's job is only to build the inputs; the shared
/// `Model::new_macro` helper (used identically by the MDL converter)
/// synthesizes the formal-parameter port variables and attaches the
/// `MacroSpec`. Port synthesis is deliberately *not* re-implemented here.
fn macro_to_datamodel(mac: Macro) -> Result<datamodel::Model> {
    // Per-macro `<sim_specs>` (a macro running with its own dt/stop time) is
    // the documented unsupported limitation. The field is parsed for
    // round-trip completeness, but a present value is rejected.
    if mac.sim_specs.is_some() {
        use crate::common::{Error, ErrorCode, ErrorKind};
        return Err(Error::new(
            ErrorKind::Import,
            ErrorCode::BadSimSpecs,
            Some(format!(
                "macro `{}` has a per-macro <sim_specs>; a macro running with \
                 its own dt/stop time is not supported",
                mac.name
            )),
        ));
    }

    let macro_name = canonicalize(&mac.name).into_owned();

    // Body: prefer an explicit <variables> body (§4.8.2); otherwise
    // (expression-form, §4.8.1) normalize the <eqn> into a macro-named body
    // variable -- the AC1.3 "expression-form <eqn> is normalized into a
    // macro-named body variable" requirement.
    //
    // Body variable idents are canonicalized to the engine ident form so
    // they are byte-identical to `MacroSpec.primary_output`/`parameters` and
    // to `Model::new_macro`'s port-matching -- exactly the invariant the MDL
    // path holds (its body variables come out of the conversion pipeline in
    // `variable_ident` form). Equation-text references are canonicalized
    // uniformly later at compile-time parse, so canonicalizing the structural
    // idents here keeps the whole macro body internally consistent.
    let mut body_variables: Vec<datamodel::Variable> = match mac.variables {
        Some(Variables { variables: vars }) => vars
            .into_iter()
            .filter(|v| !matches!(v, Var::Unhandled))
            .map(datamodel::Variable::from)
            .map(canonicalize_body_variable_idents)
            .collect(),
        None => Vec::new(),
    };

    // If no body variable already defines the primary output (the canonical
    // macro name), normalize the <eqn> into one. For the xmutil shape where
    // <variables> is present and <eqn> is literally the macro name, a body
    // variable named after the macro already exists, so this is a no-op. For
    // the expression-form (no <variables>) the <eqn> is the body expression.
    let has_primary_body = body_variables
        .iter()
        .any(|v| canonicalize(v.get_ident()) == macro_name);
    if !has_primary_body {
        let eqn = mac.eqn.clone().ok_or_else(|| {
            use crate::common::{Error, ErrorCode, ErrorKind};
            Error::new(
                ErrorKind::Import,
                ErrorCode::Generic,
                Some(format!(
                    "macro `{}` has neither a <variables> body nor an <eqn>",
                    mac.name
                )),
            )
        })?;
        body_variables.push(datamodel::Variable::Aux(datamodel::Aux {
            ident: macro_name.clone(),
            equation: datamodel::Equation::Scalar(eqn),
            documentation: mac.doc.clone().unwrap_or_default(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
    }

    let parameters: Vec<String> = mac
        .parms
        .iter()
        .map(|p| canonicalize(&p.name).into_owned())
        .collect();

    let additional_outputs: Vec<String> = mac
        .additional_outputs
        .as_ref()
        .map(|ao| {
            ao.names
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| canonicalize(s).into_owned())
                .collect()
        })
        .unwrap_or_default();

    // The shared port-synthesis + MacroSpec-construction step. Identical to
    // the MDL path -- only the way `body_variables` is produced differs.
    Ok(datamodel::Model::new_macro(
        &macro_name,
        &parameters,
        &additional_outputs,
        body_variables,
    ))
}

#[test]
fn test_xmile_roundtrips_except_equation() {
    use crate::datamodel::{Aux, Compat, Dt, Equation, SimMethod, SimSpecs, Variable};
    use std::io::BufReader;

    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: SimSpecs {
            start: 0.0,
            stop: 1.0,
            dt: Dt::Dt(1.0),
            save_step: None,
            sim_method: SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![Variable::Aux(Aux {
                ident: "test_var".to_string(),
                equation: Equation::Arrayed(
                    vec!["dim_a".to_string()],
                    vec![("a1".to_string(), "10".to_string(), None, None)],
                    Some("default_eq".to_string()),
                    true,
                ),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: Compat::default(),
                ai_state: None,
                uid: None,
            })],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: Default::default(),
        ai_information: None,
    };
    let xml = project_to_xmile(&project).unwrap();
    let roundtripped = project_from_reader(&mut BufReader::new(xml.as_bytes())).unwrap();
    let var = &roundtripped.models[0].variables[0];
    if let Variable::Aux(aux) = var {
        match &aux.equation {
            Equation::Arrayed(dims, elements, default_eq, has_except_default) => {
                assert_eq!(dims, &["dim_a"]);
                assert_eq!(elements[0].0, "a1");
                assert_eq!(elements[0].1, "10");
                assert_eq!(default_eq.as_deref(), Some("default_eq"));
                assert!(
                    *has_except_default,
                    "has_except_default must survive XMILE round-trip"
                );
            }
            other => panic!("expected Arrayed equation, got {:?}", other),
        }
    } else {
        panic!("expected Aux variable");
    }
}

#[test]
fn test_xmile_roundtrips_indexed_subdimension_parent() {
    use crate::datamodel::{DimensionElements, Dt, SimMethod, SimSpecs};
    use std::io::BufReader;

    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: SimSpecs {
            start: 0.0,
            stop: 1.0,
            dt: Dt::Dt(1.0),
            save_step: None,
            sim_method: SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![
            datamodel::Dimension {
                name: "parent_dim".to_string(),
                elements: DimensionElements::Named(vec![
                    "p1".to_string(),
                    "p2".to_string(),
                    "p3".to_string(),
                ]),
                mappings: vec![],
                parent: None,
            },
            datamodel::Dimension {
                name: "child_dim".to_string(),
                elements: DimensionElements::Indexed(2),
                mappings: vec![],
                parent: Some("parent_dim".to_string()),
            },
        ],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: Default::default(),
        ai_information: None,
    };
    let xml = project_to_xmile(&project).unwrap();
    let roundtripped = project_from_reader(&mut BufReader::new(xml.as_bytes())).unwrap();

    let child = roundtripped
        .dimensions
        .iter()
        .find(|d| d.name == "child_dim")
        .expect("child_dim must survive round-trip");
    assert_eq!(
        child.parent.as_deref(),
        Some("parent_dim"),
        "parent must survive XMILE round-trip"
    );
}

#[test]
fn test_xmile_roundtrips_element_level_dimension_mapping() {
    use crate::datamodel::{DimensionElements, DimensionMapping, Dt, SimMethod, SimSpecs};
    use std::io::BufReader;

    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: SimSpecs {
            start: 0.0,
            stop: 1.0,
            dt: Dt::Dt(1.0),
            save_step: None,
            sim_method: SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension {
            name: "dim_a".to_string(),
            elements: DimensionElements::Named(vec!["a1".to_string(), "a2".to_string()]),
            mappings: vec![DimensionMapping {
                target: "dim_b".to_string(),
                element_map: vec![
                    ("a1".to_string(), "b2".to_string()),
                    ("a2".to_string(), "b1".to_string()),
                ],
            }],
            parent: None,
        }],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: Default::default(),
        ai_information: None,
    };
    let xml = project_to_xmile(&project).unwrap();
    let roundtripped = project_from_reader(&mut BufReader::new(xml.as_bytes())).unwrap();
    let dim = &roundtripped.dimensions[0];
    assert_eq!(dim.name, "dim_a");
    assert_eq!(dim.mappings.len(), 1);
    assert_eq!(dim.mappings[0].target, "dim_b");
    assert_eq!(dim.mappings[0].element_map.len(), 2);
    assert_eq!(
        dim.mappings[0].element_map[0],
        ("a1".to_string(), "b2".to_string())
    );
    assert_eq!(
        dim.mappings[0].element_map[1],
        ("a2".to_string(), "b1".to_string())
    );
}

#[test]
fn test_xmile_roundtrips_multi_target_mappings() {
    use crate::datamodel::{DimensionElements, DimensionMapping, Dt, SimMethod, SimSpecs};
    use std::io::BufReader;

    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: SimSpecs {
            start: 0.0,
            stop: 1.0,
            dt: Dt::Dt(1.0),
            save_step: None,
            sim_method: SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension {
            name: "dim_a".to_string(),
            elements: DimensionElements::Named(vec!["a1".to_string(), "a2".to_string()]),
            mappings: vec![
                DimensionMapping {
                    target: "dim_b".to_string(),
                    element_map: vec![],
                },
                DimensionMapping {
                    target: "dim_c".to_string(),
                    element_map: vec![],
                },
            ],
            parent: None,
        }],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: Default::default(),
        ai_information: None,
    };
    let xml = project_to_xmile(&project).unwrap();
    let roundtripped = project_from_reader(&mut BufReader::new(xml.as_bytes())).unwrap();
    let dim = &roundtripped.dimensions[0];
    assert_eq!(dim.name, "dim_a");
    assert_eq!(dim.mappings.len(), 2);
    assert_eq!(dim.mappings[0].target, "dim_b");
    assert!(dim.mappings[0].element_map.is_empty());
    assert_eq!(dim.mappings[1].target, "dim_c");
    assert!(dim.mappings[1].element_map.is_empty());
}

#[test]
fn test_xmile_roundtrips_data_source() {
    use crate::datamodel::{
        Aux, Compat, DataSource, DataSourceKind, Dt, Equation, SimMethod, SimSpecs, Variable,
    };
    use std::io::BufReader;

    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: SimSpecs {
            start: 0.0,
            stop: 1.0,
            dt: Dt::Dt(1.0),
            save_step: None,
            sim_method: SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![Variable::Aux(Aux {
                ident: "data_var".to_string(),
                equation: Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: Compat {
                    data_source: Some(DataSource {
                        kind: DataSourceKind::Data,
                        file: "test.xlsx".to_string(),
                        tab_or_delimiter: "Sheet1".to_string(),
                        row_or_col: "A".to_string(),
                        cell: "B2".to_string(),
                    }),
                    ..Compat::default()
                },
                ai_state: None,
                uid: None,
            })],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: Default::default(),
        ai_information: None,
    };
    let xml = project_to_xmile(&project).unwrap();
    let roundtripped = project_from_reader(&mut BufReader::new(xml.as_bytes())).unwrap();
    let var = &roundtripped.models[0].variables[0];
    if let Variable::Aux(aux) = var {
        let ds = aux
            .compat
            .data_source
            .as_ref()
            .expect("expected data_source");
        assert_eq!(ds.kind, DataSourceKind::Data);
        assert_eq!(ds.file, "test.xlsx");
        assert_eq!(ds.tab_or_delimiter, "Sheet1");
        assert_eq!(ds.row_or_col, "A");
        assert_eq!(ds.cell, "B2");
    } else {
        panic!("expected Aux variable");
    }
}

#[test]
fn test_xmile_roundtrips_loop_metadata() {
    use crate::datamodel::{Dt, LoopMetadata, SimMethod, SimSpecs};
    use std::io::BufReader;

    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: Dt::Dt(1.0),
            save_step: None,
            sim_method: SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![],
            views: vec![],
            loop_metadata: vec![
                LoopMetadata {
                    uids: vec![1, 2, 3],
                    deleted: false,
                    name: "growth loop".to_string(),
                    description: "a reinforcing loop".to_string(),
                },
                LoopMetadata {
                    uids: vec![4, 5],
                    deleted: true,
                    name: "decay loop".to_string(),
                    description: String::new(),
                },
            ],
            groups: vec![],
            macro_spec: None,
        }],
        source: Default::default(),
        ai_information: None,
    };

    let xml = project_to_xmile(&project).unwrap();
    let roundtripped = project_from_reader(&mut BufReader::new(xml.as_bytes())).unwrap();

    assert_eq!(
        roundtripped.models[0].loop_metadata.len(),
        2,
        "loop_metadata must survive XMILE round-trip"
    );
    let lm0 = &roundtripped.models[0].loop_metadata[0];
    assert_eq!(lm0.uids, vec![1, 2, 3]);
    assert_eq!(lm0.name, "growth loop");
    assert_eq!(lm0.description, "a reinforcing loop");
    assert!(!lm0.deleted);

    let lm1 = &roundtripped.models[0].loop_metadata[1];
    assert_eq!(lm1.uids, vec![4, 5]);
    assert_eq!(lm1.name, "decay loop");
    assert!(lm1.deleted);
    assert!(lm1.description.is_empty());
}

#[cfg(test)]
mod macro_tests {
    use super::*;
    use crate::datamodel::{Equation, Variable};
    use std::io::BufReader;

    /// Find the (single) macro-marked model in a project, by macro name.
    fn macro_model<'a>(project: &'a datamodel::Project, name: &str) -> &'a datamodel::Model {
        project
            .models
            .iter()
            .find(|m| m.name == name && m.macro_spec.is_some())
            .unwrap_or_else(|| {
                panic!(
                    "expected a macro-marked model named {:?}; models: {:?}",
                    name,
                    project
                        .models
                        .iter()
                        .map(|m| (m.name.clone(), m.macro_spec.is_some()))
                        .collect::<Vec<_>>()
                )
            })
    }

    fn scalar_eq(var: &Variable) -> &str {
        match var.get_equation() {
            Some(Equation::Scalar(s)) => s.as_str(),
            other => panic!("expected Scalar equation, got {:?}", other),
        }
    }

    /// macros.AC1.3: an expression-form `<macro>` (no `<variables>`) imports
    /// as a macro-marked model whose `<eqn>` is normalized into a
    /// macro-named body variable, with synthesized parameter ports.
    #[test]
    fn expression_form_macro_imports_as_macro_marked_model() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
    <header><vendor>test</vendor><product>test</product><name>m</name></header>
    <sim_specs><start>0</start><stop>1</stop><dt>1</dt></sim_specs>
    <model><variables>
        <aux name="result"><eqn>MYMACRO(2, 3)</eqn></aux>
    </variables></model>
    <macro name="MYMACRO">
        <parm>a</parm>
        <parm>b</parm>
        <eqn>a * b</eqn>
    </macro>
</xmile>"#;
        let project =
            project_from_reader(&mut BufReader::new(xml.as_bytes())).expect("must import");

        let m = macro_model(&project, "mymacro");
        let spec = m.macro_spec.as_ref().expect("macro_spec: Some");
        assert_eq!(
            spec.parameters,
            vec!["a".to_string(), "b".to_string()],
            "MacroSpec.parameters must be the <parm> names in order"
        );
        assert_eq!(
            spec.primary_output, "mymacro",
            "primary_output must be the canonical macro name"
        );
        assert!(
            spec.additional_outputs.is_empty(),
            "single-output macro has no additional outputs"
        );

        // The <eqn> was normalized into a macro-named body variable.
        let body = m
            .variables
            .iter()
            .find(|v| v.get_ident() == "mymacro")
            .expect("a body variable named after the macro");
        assert_eq!(
            scalar_eq(body),
            "a * b",
            "the normalized <eqn> body equation"
        );

        // Synthesized parameter ports a/b with can_be_module_input == true.
        for p in ["a", "b"] {
            let port = m
                .variables
                .iter()
                .find(|v| v.get_ident() == p)
                .unwrap_or_else(|| panic!("synthesized port {:?} must exist", p));
            assert!(
                port.can_be_module_input(),
                "synthesized port {:?} must have can_be_module_input == true",
                p
            );
        }
    }

    /// macros.AC1.3: a `<macro>` with a `<variables>` body imports with the
    /// `<variables>` as the body and the `<eqn>`-named variable as
    /// `primary_output`.
    #[test]
    fn variables_body_macro_imports_with_body_and_primary_output() {
        // Mirrors the xmutil-emitted shape: <eqn> holds the macro name and
        // <variables> carries the real body equation.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
    <header><vendor>test</vendor><product>test</product><name>m</name></header>
    <sim_specs><start>0</start><stop>1</stop><dt>1</dt></sim_specs>
    <model><variables>
        <aux name="result"><eqn>EXPRESSION_MACRO(2, 3)</eqn></aux>
    </variables></model>
    <macro name="EXPRESSION MACRO">
        <eqn>EXPRESSION MACRO</eqn>
        <parm>input</parm>
        <parm>parameter</parm>
        <variables>
            <aux name="EXPRESSION MACRO">
                <doc>tests basic macro</doc>
                <eqn>input*parameter</eqn>
                <units>input</units>
            </aux>
        </variables>
    </macro>
</xmile>"#;
        let project =
            project_from_reader(&mut BufReader::new(xml.as_bytes())).expect("must import");

        let m = macro_model(&project, "expression_macro");
        let spec = m.macro_spec.as_ref().expect("macro_spec: Some");
        assert_eq!(
            spec.parameters,
            vec!["input".to_string(), "parameter".to_string()]
        );
        assert_eq!(
            spec.primary_output, "expression_macro",
            "primary_output is the canonical macro name (matches the <eqn>)"
        );

        // The <variables> body equation survives (the macro-named body var).
        let body = m
            .variables
            .iter()
            .find(|v| v.get_ident() == "expression_macro")
            .expect("the <variables> body var");
        assert_eq!(scalar_eq(body), "input*parameter");

        // Synthesized ports input/parameter.
        for p in ["input", "parameter"] {
            let port = m
                .variables
                .iter()
                .find(|v| v.get_ident() == p)
                .unwrap_or_else(|| panic!("port {:?} must exist", p));
            assert!(port.can_be_module_input());
        }
    }

    /// A `<macro>` with a non-empty `<sim_specs>` is the documented
    /// unsupported limitation: conversion returns a clear error.
    #[test]
    fn macro_with_sim_specs_is_a_documented_limitation_error() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
    <header><vendor>test</vendor><product>test</product><name>m</name></header>
    <sim_specs><start>0</start><stop>1</stop><dt>1</dt></sim_specs>
    <model><variables>
        <aux name="result"><eqn>MYMACRO(2)</eqn></aux>
    </variables></model>
    <macro name="MYMACRO">
        <parm>a</parm>
        <sim_specs><start>0</start><stop>5</stop><dt>0.5</dt></sim_specs>
        <variables>
            <aux name="MYMACRO"><eqn>a * 2</eqn></aux>
        </variables>
    </macro>
</xmile>"#;
        let err = project_from_reader(&mut BufReader::new(xml.as_bytes()))
            .expect_err("per-macro <sim_specs> must be rejected");
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("sim_specs") || msg.contains("sim specs"),
            "the error must mention sim_specs; got: {:?}",
            err.to_string()
        );
    }

    use crate::datamodel::{Aux, Compat, Dt, Model, SimMethod, SimSpecs};

    fn base_sim_specs() -> SimSpecs {
        SimSpecs {
            start: 0.0,
            stop: 1.0,
            dt: Dt::Dt(1.0),
            save_step: None,
            sim_method: SimMethod::Euler,
            time_units: None,
        }
    }

    fn aux(ident: &str, eqn: &str) -> Variable {
        Variable::Aux(Aux {
            ident: ident.to_string(),
            equation: Equation::Scalar(eqn.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        })
    }

    /// A project with one ordinary model invoking a single-output macro
    /// `mymacro(a, b) = a * b`. The macro-marked model is built via the
    /// shared `Model::new_macro` so the port synthesis matches the reader.
    fn single_output_macro_project() -> datamodel::Project {
        let macro_model = Model::new_macro(
            "mymacro",
            &["a".to_string(), "b".to_string()],
            &[],
            vec![aux("mymacro", "a * b")],
        );
        datamodel::Project {
            name: "test".to_string(),
            sim_specs: base_sim_specs(),
            dimensions: vec![],
            units: vec![],
            models: vec![
                datamodel::Model {
                    name: "main".to_string(),
                    sim_specs: None,
                    variables: vec![aux("result", "mymacro(2, 3)")],
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
                    macro_spec: None,
                },
                macro_model,
            ],
            source: Default::default(),
            ai_information: None,
        }
    }

    /// macros.AC4.2: a single-output macro-marked model serializes to a
    /// `<macro name>` element with its `<parm>`s and body, and the
    /// `<uses_macros recursive_macros="false" option_filters="false"/>`
    /// header option is emitted.
    #[test]
    fn single_output_macro_writes_macro_element_and_uses_macros_option() {
        let project = single_output_macro_project();
        let xml = project_to_xmile(&project).expect("must serialize");

        assert!(
            xml.contains(r#"<macro name="mymacro">"#),
            "expected a <macro name=\"mymacro\"> element; got:\n{}",
            xml
        );
        assert!(
            xml.contains("<parm>a</parm>"),
            "expected <parm>a</parm>; got:\n{}",
            xml
        );
        assert!(
            xml.contains("<parm>b</parm>"),
            "expected <parm>b</parm>; got:\n{}",
            xml
        );
        // <eqn> holds the primary output name (the xmutil shape).
        assert!(
            xml.contains("<eqn>mymacro</eqn>"),
            "expected <eqn>mymacro</eqn> (the primary-output name); got:\n{}",
            xml
        );
        // The body equation survives in <variables>.
        assert!(
            xml.contains("<eqn>a * b</eqn>"),
            "expected the body equation <eqn>a * b</eqn>; got:\n{}",
            xml
        );
        assert!(
            xml.contains(r#"<uses_macros recursive_macros="false" option_filters="false"/>"#),
            "expected the <uses_macros> header option; got:\n{}",
            xml
        );
    }

    /// macros.AC4.5: a single-output-only macro project exports as
    /// standards-clean XMILE with no `simlin:` macro-extension element.
    #[test]
    fn single_output_macro_emits_no_simlin_macro_extension() {
        let project = single_output_macro_project();
        let xml = project_to_xmile(&project).expect("must serialize");

        assert!(
            !xml.contains("simlin:additional-outputs"),
            "single-output macro must NOT emit the simlin:additional-outputs \
             extension; got:\n{}",
            xml
        );
        assert!(
            !xml.contains("simlin:macro-invocation"),
            "single-output macro must NOT emit the simlin:macro-invocation \
             extension; got:\n{}",
            xml
        );
    }

    /// macros.AC4.2: `to_xmile` -> `open_xmile` preserves the macro-marked
    /// model with the same `MacroSpec` and body.
    #[test]
    fn single_output_macro_to_xmile_open_xmile_round_trips() {
        let project = single_output_macro_project();
        let xml = project_to_xmile(&project).expect("must serialize");
        let roundtripped =
            project_from_reader(&mut BufReader::new(xml.as_bytes())).expect("must re-import");

        let m = macro_model(&roundtripped, "mymacro");
        let spec = m.macro_spec.as_ref().expect("macro_spec survives");
        assert_eq!(spec.parameters, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(spec.primary_output, "mymacro");
        assert!(spec.additional_outputs.is_empty());

        let body = m
            .variables
            .iter()
            .find(|v| v.get_ident() == "mymacro")
            .expect("body var survives");
        assert_eq!(scalar_eq(body), "a * b");

        // The ordinary model and its invocation are unchanged.
        let main = roundtripped
            .models
            .iter()
            .find(|m| m.name == "main")
            .expect("main model survives");
        assert!(main.macro_spec.is_none());
        let result = main
            .variables
            .iter()
            .find(|v| v.get_ident() == "result")
            .expect("result var survives");
        assert_eq!(scalar_eq(result), "mymacro(2, 3)");

        // A second serialization is byte-identical (byte-stable round-trip).
        let xml2 = project_to_xmile(&roundtripped).expect("must re-serialize");
        assert_eq!(
            xml, xml2,
            "single-output macro round-trip must be byte-stable"
        );
    }

    use crate::datamodel::{Module, ModuleReference};

    /// A project with a multi-output macro `add3(a,b,c : minval, maxval)` and
    /// the Phase-4-materialized invocation cluster
    /// `total = add3(in1,in2,in3 : the min, the max)`.
    fn multi_output_macro_project() -> datamodel::Project {
        let macro_model = Model::new_macro(
            "add3",
            &["a".to_string(), "b".to_string(), "c".to_string()],
            &["minval".to_string(), "maxval".to_string()],
            vec![
                aux("add3", "a + b + c"),
                aux("minval", "MIN(a, MIN(b, c))"),
                aux("maxval", "MAX(a, MAX(b, c))"),
            ],
        );

        let module = Variable::Module(Module {
            ident: "total_macro".to_string(),
            model_name: "add3".to_string(),
            documentation: String::new(),
            units: None,
            references: vec![
                ModuleReference {
                    src: "in1".to_string(),
                    dst: "total_macro.a".to_string(),
                },
                ModuleReference {
                    src: "in2".to_string(),
                    dst: "total_macro.b".to_string(),
                },
                ModuleReference {
                    src: "in3".to_string(),
                    dst: "total_macro.c".to_string(),
                },
            ],
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });

        datamodel::Project {
            name: "test".to_string(),
            sim_specs: base_sim_specs(),
            dimensions: vec![],
            units: vec![],
            models: vec![
                datamodel::Model {
                    name: "main".to_string(),
                    sim_specs: None,
                    variables: vec![
                        aux("in1", "7"),
                        aux("in2", "2"),
                        aux("in3", "5"),
                        module,
                        // primary-output binding (replaces the LHS aux)
                        aux("total", "total_macro.add3"),
                        // additional-output bindings
                        aux("the_min", "total_macro.minval"),
                        aux("the_max", "total_macro.maxval"),
                        aux("spread", "the_max - the_min"),
                    ],
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
                    macro_spec: None,
                },
                macro_model,
            ],
            source: Default::default(),
            ai_information: None,
        }
    }

    /// Find the (single) Variable::Module in a model.
    fn the_module(model: &datamodel::Model) -> &Module {
        let mods: Vec<&Module> = model
            .variables
            .iter()
            .filter_map(|v| match v {
                Variable::Module(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(
            mods.len(),
            1,
            "expected exactly one Variable::Module in {:?}; vars: {:?}",
            model.name,
            model
                .variables
                .iter()
                .map(|v| v.get_ident().to_string())
                .collect::<Vec<_>>()
        );
        mods[0]
    }

    /// macros.AC4.5: a multi-output macro triggers BOTH simlin: extensions
    /// (the additional-outputs on the <macro> and the multi-output
    /// invocation), and a single-output project triggers NEITHER.
    #[test]
    fn multi_output_macro_emits_both_simlin_extensions() {
        let project = multi_output_macro_project();
        let xml = project_to_xmile(&project).expect("must serialize");

        assert!(
            xml.contains(r#"<simlin:additional-outputs names="minval,maxval"/>"#),
            "multi-output macro must emit the simlin:additional-outputs \
             extension; got:\n{}",
            xml
        );
        assert!(
            xml.contains("simlin:macro-invocation"),
            "multi-output invocation must emit the simlin:macro-invocation \
             extension; got:\n{}",
            xml
        );

        // Contrast: a single-output project emits neither.
        let single = project_to_xmile(&single_output_macro_project()).unwrap();
        assert!(!single.contains("simlin:additional-outputs"));
        assert!(!single.contains("simlin:macro-invocation"));
    }

    /// macros.AC4.2: a multi-output macro project round-trips through the
    /// simlin: extensions -- same MacroSpec.additional_outputs, same
    /// materialized Variable::Module + binding Aux-es, and a second
    /// serialization is byte-identical.
    #[test]
    fn multi_output_macro_round_trips_through_simlin_extensions() {
        let project = multi_output_macro_project();
        let xml = project_to_xmile(&project).expect("must serialize");
        let rt = project_from_reader(&mut BufReader::new(xml.as_bytes())).expect("must re-import");

        // The macro keeps its 2-additional-output spec.
        let m = macro_model(&rt, "add3");
        let spec = m.macro_spec.as_ref().expect("macro_spec survives");
        assert_eq!(
            spec.parameters,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert_eq!(spec.primary_output, "add3");
        assert_eq!(
            spec.additional_outputs,
            vec!["minval".to_string(), "maxval".to_string()],
            "MacroSpec.additional_outputs must survive the simlin: extension"
        );

        // The materialized Variable::Module is reconstructed exactly.
        let main = rt.models.iter().find(|m| m.name == "main").unwrap();
        let module = the_module(main);
        assert_eq!(module.ident, "total_macro");
        assert_eq!(module.model_name, "add3");
        let mut refs: Vec<(String, String)> = module
            .references
            .iter()
            .map(|r| (r.src.clone(), r.dst.clone()))
            .collect();
        refs.sort();
        assert_eq!(
            refs,
            vec![
                ("in1".to_string(), "total_macro.a".to_string()),
                ("in2".to_string(), "total_macro.b".to_string()),
                ("in3".to_string(), "total_macro.c".to_string()),
            ]
        );

        // The binding auxes are reconstructed exactly (ASCII period).
        let total = main.variables.iter().find(|v| v.get_ident() == "total");
        assert_eq!(
            scalar_eq(total.expect("total survives")),
            "total_macro.add3"
        );
        let the_min = main.variables.iter().find(|v| v.get_ident() == "the_min");
        assert_eq!(
            scalar_eq(the_min.expect("the_min survives")),
            "total_macro.minval"
        );
        let the_max = main.variables.iter().find(|v| v.get_ident() == "the_max");
        assert_eq!(
            scalar_eq(the_max.expect("the_max survives")),
            "total_macro.maxval"
        );
        // The unrelated downstream var is untouched.
        let spread = main.variables.iter().find(|v| v.get_ident() == "spread");
        assert_eq!(
            scalar_eq(spread.expect("spread survives")),
            "the_max - the_min"
        );

        // Second serialization is byte-identical (byte-stable round-trip).
        let xml2 = project_to_xmile(&rt).expect("must re-serialize");
        assert_eq!(xml, xml2, "multi-output round-trip must be byte-stable");
    }

    /// macros.AC4.4: a multi-output `:`-form `.mdl` survives a cross-format
    /// conversion `.mdl` -> datamodel -> `.xmile` -> datamodel.
    #[test]
    fn multi_output_cross_format_mdl_to_xmile_round_trips() {
        const MDL: &str = include_str!(
            "../../../../test/test-models/tests/macro_multi_output/test_macro_multi_output.mdl"
        );
        let from_mdl = crate::compat::open_vensim(MDL).expect("macro_multi_output .mdl imports");

        let xml = project_to_xmile(&from_mdl).expect("must serialize to XMILE");
        let rt = project_from_reader(&mut BufReader::new(xml.as_bytes()))
            .expect("the XMILE must re-import");

        // The ADD3 macro definition survives with its multi-output spec.
        let m = macro_model(&rt, "add3");
        let spec = m.macro_spec.as_ref().expect("macro_spec survives");
        assert_eq!(
            spec.parameters,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert_eq!(
            spec.additional_outputs,
            vec!["minval".to_string(), "maxval".to_string()]
        );

        // The invocation survives as the materialized module + bindings.
        let main = rt.models.iter().find(|m| m.name == "main").unwrap();
        let module = the_module(main);
        assert_eq!(module.model_name, "add3");
        let total = main.variables.iter().find(|v| v.get_ident() == "total");
        assert_eq!(
            scalar_eq(total.expect("total survives")),
            format!("{}.add3", module.ident)
        );
    }
}
