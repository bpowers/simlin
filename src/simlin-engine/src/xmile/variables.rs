// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use quick_xml::Writer;
use serde::{Deserialize, Serialize};

use crate::common::{Result, canonicalize};
use crate::datamodel;
use crate::datamodel::Equation;
use crate::xmile::dimensions::Gf;
use crate::xmile::{
    ToXml, VarDimension, VarDimensions, XmlWriter, write_tag, write_tag_empty_with_attrs,
    write_tag_end, write_tag_start, write_tag_start_with_attrs,
};

use super::model::{Module, NonNegative, access_from, can_be_module_input, visibility};

/// Vendor extension element for external data source metadata.
/// Serialized as `<simlin:data_source kind="..." file="..." .../>`.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DataSourceElement {
    #[serde(rename = "@kind")]
    pub kind: String,
    #[serde(rename = "@file")]
    pub file: String,
    #[serde(rename = "@tab", default)]
    pub tab: Option<String>,
    #[serde(rename = "@row_or_col", default)]
    pub row_or_col: Option<String>,
    #[serde(rename = "@cell", default)]
    pub cell: Option<String>,
}

impl DataSourceElement {
    fn from_datamodel(ds: &datamodel::DataSource) -> Self {
        let kind = match ds.kind {
            datamodel::DataSourceKind::Data => "data",
            datamodel::DataSourceKind::Constants => "constants",
            datamodel::DataSourceKind::Lookups => "lookups",
            datamodel::DataSourceKind::Subscript => "subscript",
        };
        DataSourceElement {
            kind: kind.to_string(),
            file: ds.file.clone(),
            tab: if ds.tab_or_delimiter.is_empty() {
                None
            } else {
                Some(ds.tab_or_delimiter.clone())
            },
            row_or_col: if ds.row_or_col.is_empty() {
                None
            } else {
                Some(ds.row_or_col.clone())
            },
            cell: if ds.cell.is_empty() {
                None
            } else {
                Some(ds.cell.clone())
            },
        }
    }

    fn to_datamodel(&self) -> datamodel::DataSource {
        let kind = match self.kind.as_str() {
            "constants" => datamodel::DataSourceKind::Constants,
            "lookups" => datamodel::DataSourceKind::Lookups,
            "subscript" => datamodel::DataSourceKind::Subscript,
            _ => datamodel::DataSourceKind::Data,
        };
        datamodel::DataSource {
            kind,
            file: self.file.clone(),
            tab_or_delimiter: self.tab.clone().unwrap_or_default(),
            row_or_col: self.row_or_col.clone().unwrap_or_default(),
            cell: self.cell.clone().unwrap_or_default(),
        }
    }
}

impl ToXml<XmlWriter> for DataSourceElement {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        let mut attrs = vec![("kind", self.kind.as_str()), ("file", self.file.as_str())];
        let tab_ref;
        if let Some(ref tab) = self.tab {
            tab_ref = tab.as_str();
            attrs.push(("tab", tab_ref));
        }
        let roc_ref;
        if let Some(ref roc) = self.row_or_col {
            roc_ref = roc.as_str();
            attrs.push(("row_or_col", roc_ref));
        }
        let cell_ref;
        if let Some(ref cell) = self.cell {
            cell_ref = cell.as_str();
            attrs.push(("cell", cell_ref));
        }
        super::write_tag_with_attrs(writer, "simlin:data_source", "", &attrs)
    }
}

/// One `<element>` block of a non-apply-to-all arrayed variable. XMILE 4.5.2
/// does NOT require an `<eqn>` child: an element may carry just a graphical
/// function (Stella's non-A2A gf export) or other per-element attributes, so
/// `eqn` is optional (GH #907). An absent eqn converts to an empty element
/// equation in the datamodel -- the same "no functional input" encoding a
/// whole lookup-only variable uses (`variable::is_lookup_only`).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct VarElement {
    #[serde(rename = "@subscript")]
    pub subscript: String,
    pub eqn: Option<String>,
    #[serde(rename = "init_eqn")]
    pub initial_eqn: Option<String>,
    pub gf: Option<Gf>,
}

impl ToXml<XmlWriter> for VarElement {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        let attrs = &[("subscript", self.subscript.as_str())];
        write_tag_start_with_attrs(writer, "element", attrs)?;
        if let Some(eqn) = &self.eqn {
            write_tag(writer, "eqn", eqn.as_str())?;
        }
        if let Some(init_eqn) = &self.initial_eqn {
            write_tag(writer, "init_eqn", init_eqn.as_str())?;
        }
        if let Some(gf) = &self.gf {
            gf.write_xml(writer)?;
        }
        write_tag_end(writer, "element")
    }
}

/// The `<conveyor>` block on a conveyor stock. See docs/design/conveyors.md §3.2.
/// The `<len>` transit time is required; the rest are optional. Boolean sub-feature
/// attributes are `Option<bool>` so an absent attribute maps to the documented
/// default (not blindly `false`) during datamodel conversion.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct Conveyor {
    pub len: String,
    pub capacity: Option<String>,
    pub in_limit: Option<String>,
    pub sample: Option<String>,
    pub arrest: Option<String>,
    #[serde(rename = "@discrete")]
    pub discrete: Option<bool>,
    #[serde(rename = "@batch_integrity")]
    pub batch_integrity: Option<bool>,
    #[serde(rename = "@one_at_a_time")]
    pub one_at_a_time: Option<bool>,
    #[serde(rename = "@exponential_leak")]
    pub exponential_leak: Option<bool>,
}

impl ToXml<XmlWriter> for Conveyor {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        // Emit an attribute only when it differs from the XMILE default, so a
        // plain `<conveyor>` round-trips without spurious attributes.
        let mut attrs: Vec<(&str, &str)> = vec![];
        if self.discrete == Some(true) {
            attrs.push(("discrete", "true"));
        }
        if self.batch_integrity == Some(true) {
            attrs.push(("batch_integrity", "true"));
        }
        if self.one_at_a_time == Some(false) {
            attrs.push(("one_at_a_time", "false"));
        }
        if self.exponential_leak == Some(true) {
            attrs.push(("exponential_leak", "true"));
        }
        write_tag_start_with_attrs(writer, "conveyor", &attrs)?;
        write_tag(writer, "len", &self.len)?;
        if let Some(ref c) = self.capacity {
            write_tag(writer, "capacity", c)?;
        }
        if let Some(ref v) = self.in_limit {
            write_tag(writer, "in_limit", v)?;
        }
        if let Some(ref v) = self.sample {
            write_tag(writer, "sample", v)?;
        }
        if let Some(ref v) = self.arrest {
            write_tag(writer, "arrest", v)?;
        }
        write_tag_end(writer, "conveyor")
    }
}

/// The `<queue/>` marker on a queue stock. A bare marker with no options
/// (XMILE §4.2; docs/design/queues.md §3.2). Present iff the stock is a queue.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct Queue {}

/// A `<leak>` element on a conveyor outflow. A bare `<leak/>` marker deserializes
/// to `value: None` (the fraction is then carried by the flow's `<eqn>`); a
/// value-bearing `<leak>expr</leak>` deserializes to `value: Some(expr)`.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct Leak {
    #[serde(rename = "$text")]
    pub value: Option<String>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
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
    pub conveyor: Option<Conveyor>,
    pub queue: Option<Queue>,
    pub dimensions: Option<VarDimensions>,
    #[serde(rename = "element", default)]
    pub elements: Option<Vec<VarElement>>,
    #[serde(rename = "@access")]
    pub access: Option<String>,
    #[serde(rename = "@ai_state")]
    pub ai_state: Option<String>,
    // quick-xml strips namespace prefixes during deserialization
    #[serde(rename = "data_source")]
    pub data_source: Option<DataSourceElement>,
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

        if let Some(ref conveyor) = self.conveyor {
            conveyor.write_xml(writer)?;
        }

        if self.queue.is_some() {
            write_tag_empty_with_attrs(writer, "queue", &[])?;
        }

        if let Some(ref ai_state) = self.ai_state {
            write_tag(writer, "ai_state", ai_state)?;
        }

        if let Some(ref ds) = self.data_source {
            ds.write_xml(writer)?;
        }

        write_tag_end(writer, "stock")
    }
}

/// Convert an XMILE `<conveyor>` block to the datamodel, applying documented
/// defaults for absent boolean attributes (`one_at_a_time` defaults to true).
fn conveyor_to_datamodel(c: Conveyor) -> datamodel::Conveyor {
    datamodel::Conveyor {
        transit_time: c.len,
        capacity: c.capacity,
        inflow_limit: c.in_limit,
        sample: c.sample,
        arrest: c.arrest,
        discrete: c.discrete.unwrap_or(false),
        batch_integrity: c.batch_integrity.unwrap_or(false),
        one_at_a_time: c.one_at_a_time.unwrap_or(true),
        exponential_leak: c.exponential_leak.unwrap_or(false),
        // The isee "ignore earlier zone losses" toggle is persisted as an isee
        // vendor attribute whose exact name is unconfirmed (no vendored fixture
        // uses it); default to false until a real file pins the spelling.
        ignore_earlier_zone_losses: false,
    }
}

/// Convert a datamodel conveyor back to the XMILE serde form, dropping default
/// boolean flags to `None` so `write_xml` only emits non-default attributes.
fn conveyor_from_datamodel(c: datamodel::Conveyor) -> Conveyor {
    Conveyor {
        len: c.transit_time,
        capacity: c.capacity,
        in_limit: c.inflow_limit,
        sample: c.sample,
        arrest: c.arrest,
        discrete: if c.discrete { Some(true) } else { None },
        batch_integrity: if c.batch_integrity { Some(true) } else { None },
        one_at_a_time: if c.one_at_a_time { None } else { Some(false) },
        exponential_leak: if c.exponential_leak { Some(true) } else { None },
    }
}

macro_rules! convert_equation(
    ($var:expr) => {{
        if let Some(elements) = $var.elements {
            let dimensions = match $var.dimensions {
                Some(dimensions) => dimensions.dimensions.unwrap().into_iter().map(|e| canonicalize(&e.name).into_owned()).collect(),
                None => vec![],
            };
            let elements = elements.into_iter().map(|e| {
                let canonical_subscripts: Vec<_> = e.subscript.split(",").map(|s| canonicalize(s.trim()).into_owned()).collect();
                // An eqn-less element (e.g. gf-only, GH #907) gets an empty
                // equation: with a gf that makes the element a per-element
                // lookup table; the writer re-emits it without an <eqn> tag.
                (canonical_subscripts.join(","), e.eqn.unwrap_or_default(), e.initial_eqn, e.gf.map(datamodel::GraphicalFunction::from))
            }).collect();
            // When a top-level <eqn> coexists with <element> entries, the
            // top-level eqn is the EXCEPT default equation.
            let default_equation = $var.eqn.filter(|s| !s.is_empty());
            let has_except_default = default_equation.is_some();
            datamodel::Equation::Arrayed(dimensions, elements, default_equation, has_except_default)
        } else if let Some(dimensions) = $var.dimensions {
            let dimensions = dimensions.dimensions.unwrap_or_default().into_iter().map(|e| canonicalize(&e.name).into_owned()).collect();
            datamodel::Equation::ApplyToAll(dimensions, $var.eqn.unwrap_or_default())
        } else {
            datamodel::Equation::Scalar($var.eqn.unwrap_or_default())
        }
    }}
);

macro_rules! extract_compat(
    ($var:expr, $access:expr) => {{
        let active_initial = $var.initial_eqn.filter(|s| !s.is_empty());
        datamodel::Compat {
            active_initial,
            can_be_module_input: can_be_module_input(&$access),
            visibility: visibility(&$access),
            ..Default::default()
        }
    }}
);

pub(crate) fn ai_state_from(s: Option<String>) -> Option<datamodel::AiState> {
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
            .map(|id| canonicalize(&id).into_owned())
            .collect();
        let outflows = stock
            .outflows
            .unwrap_or_default()
            .into_iter()
            .map(|id| canonicalize(&id).into_owned())
            .collect();
        let data_source = stock.data_source.as_ref().map(|ds| ds.to_datamodel());
        datamodel::Stock {
            ident: stock.name.clone(),
            equation: convert_equation!(stock),
            documentation: stock.doc.unwrap_or_default(),
            units: stock.units,
            inflows,
            outflows,
            compat: datamodel::Compat {
                non_negative: stock.non_negative.is_some(),
                can_be_module_input: can_be_module_input(&stock.access),
                visibility: visibility(&stock.access),
                data_source,
                conveyor: stock.conveyor.map(conveyor_to_datamodel),
                queue: stock.queue.map(|_| datamodel::Queue {}),
                ..datamodel::Compat::default()
            },
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
                // Only write the default equation to <eqn> when it's an active
                // EXCEPT default; otherwise the XMILE importer would infer
                // has_except_default=true on reimport and change model behavior.
                Equation::Arrayed(_, _, default_eq, has_except) => {
                    if *has_except {
                        default_eq.clone()
                    } else {
                        None
                    }
                }
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
            non_negative: if stock.compat.non_negative {
                Some(NonNegative {})
            } else {
                None
            },
            conveyor: stock.compat.conveyor.map(conveyor_from_datamodel),
            queue: if stock.compat.queue.is_some() {
                Some(Queue {})
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
                Equation::Arrayed(dims, _, _, _) => Some(VarDimensions {
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
                Equation::Arrayed(_, elements, _, _) => Some(
                    elements
                        .into_iter()
                        .map(|(subscript, eqn, _, gf)| VarElement {
                            subscript,
                            // an empty element equation (gf-only element)
                            // round-trips as NO <eqn> tag, mirroring the
                            // whole-variable empty-eqn handling above
                            eqn: if eqn.is_empty() { None } else { Some(eqn) },
                            initial_eqn: None,
                            gf: gf.map(Gf::from),
                        })
                        .collect(),
                ),
            },
            access: access_from(stock.compat.visibility, stock.compat.can_be_module_input),
            ai_state: None, // TODO
            data_source: stock
                .compat
                .data_source
                .as_ref()
                .map(DataSourceElement::from_datamodel),
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
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
    // A bare `<overflow/>` marker: this flow is a queue overflow outflow, active
    // only when a higher-priority outflow is blocked (docs/design/queues.md §3.3).
    pub overflow: Option<NonNegative>,
    // Conveyor leakage: a `<leak>`/`<leak/>` marks this flow as a leakage
    // outflow; `<leak_integers/>` restricts it to whole units; the leak zone is
    // given by the `leak_start`/`leak_end` attributes on the flow.
    pub leak: Option<Leak>,
    #[serde(rename = "leak_integers")]
    pub leak_integers: Option<NonNegative>,
    #[serde(rename = "@leak_start")]
    pub leak_start: Option<String>,
    #[serde(rename = "@leak_end")]
    pub leak_end: Option<String>,
    // isee inflow placement (namespace prefix stripped by quick-xml on read).
    #[serde(rename = "@spreadflow")]
    pub spreadflow: Option<String>,
    #[serde(rename = "distrib_eq")]
    pub distrib_eq: Option<String>,
    pub dimensions: Option<VarDimensions>,
    #[serde(rename = "element", default)]
    pub elements: Option<Vec<VarElement>>,
    #[serde(rename = "@access")]
    pub access: Option<String>,
    #[serde(rename = "@ai_state")]
    pub ai_state: Option<String>,
    // quick-xml strips namespace prefixes during deserialization
    #[serde(rename = "data_source")]
    pub data_source: Option<DataSourceElement>,
}

impl ToXml<XmlWriter> for Flow {
    fn write_xml(&self, writer: &mut Writer<XmlWriter>) -> Result<()> {
        let mut attrs = vec![("name", self.name.as_str())];
        if let Some(access) = self.access.as_ref() {
            attrs.push(("access", access.as_str()));
        }
        if let Some(spreadflow) = self.spreadflow.as_ref() {
            attrs.push(("isee:spreadflow", spreadflow.as_str()));
        }
        if let Some(leak_start) = self.leak_start.as_ref() {
            attrs.push(("leak_start", leak_start.as_str()));
        }
        if let Some(leak_end) = self.leak_end.as_ref() {
            attrs.push(("leak_end", leak_end.as_str()));
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

        if self.overflow.is_some() {
            write_tag_empty_with_attrs(writer, "overflow", &[])?;
        }

        if let Some(ref leak) = self.leak {
            match &leak.value {
                Some(value) if !value.is_empty() => write_tag(writer, "leak", value)?,
                _ => write_tag_empty_with_attrs(writer, "leak", &[])?,
            }
        }
        if self.leak_integers.is_some() {
            write_tag_empty_with_attrs(writer, "leak_integers", &[])?;
        }
        if let Some(ref distrib_eq) = self.distrib_eq {
            write_tag(writer, "isee:distrib_eq", distrib_eq)?;
        }

        if let Some(ref ai_state) = self.ai_state {
            write_tag(writer, "ai_state", ai_state)?;
        }

        if let Some(ref ds) = self.data_source {
            ds.write_xml(writer)?;
        }

        write_tag_end(writer, "flow")
    }
}

/// Parse an `isee:spreadflow` attribute into the datamodel enum. Unknown values
/// fall back to `Beginning` (the XMILE default placement).
fn spreadflow_to_datamodel(method: &str, distrib_eq: Option<String>) -> datamodel::SpreadFlow {
    use datamodel::SpreadFlow::*;
    match method.trim().to_lowercase().as_str() {
        "even" => Even,
        "dest" => Dest,
        "dist" => Dist(distrib_eq.unwrap_or_default()),
        "source" => Source,
        _ => Beginning,
    }
}

impl From<Flow> for datamodel::Flow {
    fn from(flow: Flow) -> Self {
        let mut compat = extract_compat!(flow, flow.access);
        compat.non_negative = flow.non_negative.is_some();
        compat.overflow = flow.overflow.is_some();
        compat.data_source = flow.data_source.as_ref().map(|ds| ds.to_datamodel());
        if flow.leak.is_some() {
            // A value-bearing `<leak>expr</leak>` supplies the fraction; a bare
            // `<leak/>` leaves it None and the runtime falls back to the flow's
            // `<eqn>` (docs/design/conveyors.md §3.3).
            let fraction = flow
                .leak
                .as_ref()
                .and_then(|l| l.value.clone())
                .filter(|s| !s.is_empty());
            compat.leakage = Some(datamodel::Leakage {
                fraction,
                integers: flow.leak_integers.is_some(),
                zone_start: flow.leak_start.clone(),
                zone_end: flow.leak_end.clone(),
            });
        }
        if let Some(ref method) = flow.spreadflow {
            compat.spreadflow = Some(spreadflow_to_datamodel(method, flow.distrib_eq.clone()));
        }
        datamodel::Flow {
            ident: flow.name.clone(),
            equation: convert_equation!(flow),
            documentation: flow.doc.unwrap_or_default(),
            units: flow.units,
            gf: flow.gf.map(datamodel::GraphicalFunction::from),
            compat,
            ai_state: ai_state_from(flow.ai_state),
            uid: None,
        }
    }
}

impl From<datamodel::Flow> for Flow {
    fn from(flow: datamodel::Flow) -> Self {
        let (leak, leak_integers, leak_start, leak_end) = match flow.compat.leakage {
            Some(l) => (
                Some(Leak { value: l.fraction }),
                if l.integers {
                    Some(NonNegative {})
                } else {
                    None
                },
                l.zone_start,
                l.zone_end,
            ),
            None => (None, None, None, None),
        };
        let (spreadflow, distrib_eq) = match flow.compat.spreadflow {
            Some(datamodel::SpreadFlow::Beginning) => (Some("beginning".to_string()), None),
            Some(datamodel::SpreadFlow::Even) => (Some("even".to_string()), None),
            Some(datamodel::SpreadFlow::Dest) => (Some("dest".to_string()), None),
            Some(datamodel::SpreadFlow::Dist(eq)) => (Some("dist".to_string()), Some(eq)),
            Some(datamodel::SpreadFlow::Source) => (Some("source".to_string()), None),
            None => (None, None),
        };
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
                // Only write the default equation to <eqn> when it's an active
                // EXCEPT default; otherwise the XMILE importer would infer
                // has_except_default=true on reimport and change model behavior.
                Equation::Arrayed(_, _, default_eq, has_except) => {
                    if *has_except {
                        default_eq.clone()
                    } else {
                        None
                    }
                }
            },
            initial_eqn: flow.compat.active_initial,
            doc: if flow.documentation.is_empty() {
                None
            } else {
                Some(flow.documentation)
            },
            units: flow.units,
            gf: flow.gf.map(Gf::from),
            non_negative: if flow.compat.non_negative {
                Some(NonNegative {})
            } else {
                None
            },
            overflow: if flow.compat.overflow {
                Some(NonNegative {})
            } else {
                None
            },
            leak,
            leak_integers,
            leak_start,
            leak_end,
            spreadflow,
            distrib_eq,
            dimensions: match &flow.equation {
                Equation::Scalar(..) => None,
                Equation::ApplyToAll(dims, ..) => Some(VarDimensions {
                    dimensions: Some(
                        dims.iter()
                            .map(|name| VarDimension { name: name.clone() })
                            .collect(),
                    ),
                }),
                Equation::Arrayed(dims, _, _, _) => Some(VarDimensions {
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
                Equation::Arrayed(_, elements, _, _) => Some(
                    elements
                        .into_iter()
                        .map(|(subscript, eqn, initial_eqn, gf)| VarElement {
                            subscript,
                            // empty element equation -> no <eqn> tag (gf-only)
                            eqn: if eqn.is_empty() { None } else { Some(eqn) },
                            initial_eqn,
                            gf: gf.map(Gf::from),
                        })
                        .collect(),
                ),
            },
            access: access_from(flow.compat.visibility, flow.compat.can_be_module_input),
            ai_state: None, // TODO
            data_source: flow
                .compat
                .data_source
                .as_ref()
                .map(DataSourceElement::from_datamodel),
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
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
    // quick-xml strips namespace prefixes during deserialization
    #[serde(rename = "data_source")]
    pub data_source: Option<DataSourceElement>,
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

        if let Some(ref ds) = self.data_source {
            ds.write_xml(writer)?;
        }

        write_tag_end(writer, "aux")
    }
}

impl From<Aux> for datamodel::Aux {
    fn from(aux: Aux) -> Self {
        let mut compat = extract_compat!(aux, aux.access);
        compat.data_source = aux.data_source.as_ref().map(|ds| ds.to_datamodel());
        datamodel::Aux {
            ident: aux.name.clone(),
            equation: convert_equation!(aux),
            documentation: aux.doc.unwrap_or_default(),
            units: aux.units,
            gf: aux.gf.map(datamodel::GraphicalFunction::from),
            compat,
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
                // Only write the default equation to <eqn> when it's an active
                // EXCEPT default; otherwise the XMILE importer would infer
                // has_except_default=true on reimport and change model behavior.
                Equation::Arrayed(_, _, default_eq, has_except) => {
                    if *has_except {
                        default_eq.clone()
                    } else {
                        None
                    }
                }
            },
            initial_eqn: aux.compat.active_initial,
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
                Equation::Arrayed(dims, _, _, _) => Some(VarDimensions {
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
                Equation::Arrayed(_, elements, _, _) => Some(
                    elements
                        .into_iter()
                        .map(|(subscript, eqn, initial_eqn, gf)| VarElement {
                            subscript,
                            // empty element equation -> no <eqn> tag (gf-only)
                            eqn: if eqn.is_empty() { None } else { Some(eqn) },
                            initial_eqn,
                            gf: gf.map(Gf::from),
                        })
                        .collect(),
                ),
            },
            access: access_from(aux.compat.visibility, aux.compat.can_be_module_input),
            ai_state: None, // TODO
            data_source: aux
                .compat
                .data_source
                .as_ref()
                .map(DataSourceElement::from_datamodel),
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
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
        conveyor: None,
        queue: None,
        dimensions: None,
        elements: None,
        access: None,
        ai_state: None,
        data_source: None,
    });

    let expected = datamodel::Variable::Stock(datamodel::Stock {
        ident: "Heat Loss To Room".to_string(),
        equation: Equation::Scalar("total_population".to_string()),
        documentation: "People who can contract the disease.".to_string(),
        units: Some("people".to_string()),
        inflows: vec!["solar_radiation".to_string()],
        outflows: vec!["succumbing".to_string(), "succumbing_2".to_string()],
        compat: datamodel::Compat::default(),
        ai_state: None,
        uid: None,
    });

    let output = datamodel::Variable::from(input);

    assert_eq!(expected, output);
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
        conveyor: None,
        queue: None,
        dimensions: None,
        elements: None,
        access: None,
        ai_state: None,
        data_source: None,
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
        data_source: None,
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
    use crate::xmile::dimensions::{Gf, GraphicalFunctionScale};

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
        data_source: None,
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
fn test_gf_only_element_parsing() {
    // XMILE 4.5.2 permits an <element> with no <eqn> child -- e.g. the
    // gf-only per-element definitions Stella exports for non-A2A graphical
    // functions (GH #907). The reader must not fail the whole-file parse on
    // them; an absent per-element eqn maps to an empty element equation in
    // the datamodel (the same "no functional input" encoding a whole
    // lookup-only variable uses).
    let input = r#"<aux name="c">
        <element subscript="A1">
            <gf>
                <xpts>0,1</xpts>
                <ypts>10,20</ypts>
            </gf>
        </element>
        <element subscript="A2">
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
    let aux: Var = de::from_reader(input.as_bytes())
        .expect("a gf-only <element> (no <eqn>) must deserialize (GH #907)");

    let aux = if let Var::Aux(aux) = aux {
        aux
    } else {
        panic!("not an aux");
    };

    let aux = datamodel::Aux::from(aux);
    match &aux.equation {
        Equation::Arrayed(dims, elements, default_eq, has_except) => {
            assert_eq!(&vec!["dima".to_string()], dims);
            assert_eq!(&None, default_eq);
            assert!(!*has_except);
            assert_eq!(2, elements.len());
            for (subscript, eqn, initial_eqn, gf) in elements {
                assert!(
                    eqn.is_empty(),
                    "absent <eqn> on element {subscript} must map to an empty equation"
                );
                assert_eq!(&None, initial_eqn);
                assert!(gf.is_some(), "element {subscript} must keep its gf");
            }
        }
        _ => panic!("expected an Arrayed equation"),
    }
}

#[test]
fn test_module_parsing() {
    use super::model::{Connect, Module, Reference};

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

#[cfg(test)]
mod conveyor_tests {
    use crate::datamodel::{self, SpreadFlow};
    use crate::xmile::{project_from_reader, project_to_xmile};
    use std::io::BufReader;

    /// Wrap variable XML in a minimal, valid XMILE project document.
    fn wrap(vars: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options>
  </header>
  <sim_specs method="Euler" time_units="Months">
    <start>0</start><stop>12</stop><dt>0.25</dt>
  </sim_specs>
  <model><variables>{vars}</variables></model>
</xmile>"#
        )
    }

    fn parse(xml: &str) -> datamodel::Project {
        project_from_reader(&mut BufReader::new(xml.as_bytes())).expect("parse")
    }

    /// Parse, serialize, re-parse: the datamodel must survive a full round-trip.
    fn roundtrip(project: &datamodel::Project) -> datamodel::Project {
        let xml = project_to_xmile(project).expect("serialize");
        parse(&xml)
    }

    fn find_stock<'a>(p: &'a datamodel::Project, name: &str) -> &'a datamodel::Stock {
        p.models[0]
            .variables
            .iter()
            .find_map(|v| match v {
                datamodel::Variable::Stock(s) if s.ident == name => Some(s),
                _ => None,
            })
            .unwrap_or_else(|| panic!("stock {name} not found"))
    }

    fn find_flow<'a>(p: &'a datamodel::Project, name: &str) -> &'a datamodel::Flow {
        p.models[0]
            .variables
            .iter()
            .find_map(|v| match v {
                datamodel::Variable::Flow(f) if f.ident == name => Some(f),
                _ => None,
            })
            .unwrap_or_else(|| panic!("flow {name} not found"))
    }

    #[test]
    fn conveyor_block_all_fields_roundtrip() {
        let xml = wrap(
            r#"
        <stock name="belt">
          <eqn>1000</eqn>
          <inflow>in_f</inflow>
          <outflow>out_f</outflow>
          <conveyor discrete="true" one_at_a_time="false" batch_integrity="true">
            <len>4</len>
            <capacity>1200</capacity>
            <in_limit>500</in_limit>
            <sample>1</sample>
            <arrest>0</arrest>
          </conveyor>
        </stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
        );
        let p = parse(&xml);
        let check = |p: &datamodel::Project| {
            let c = find_stock(p, "belt")
                .compat
                .conveyor
                .as_ref()
                .expect("conveyor");
            assert_eq!(c.transit_time, "4");
            assert_eq!(c.capacity.as_deref(), Some("1200"));
            assert_eq!(c.inflow_limit.as_deref(), Some("500"));
            assert_eq!(c.sample.as_deref(), Some("1"));
            assert_eq!(c.arrest.as_deref(), Some("0"));
            assert!(c.discrete);
            assert!(c.batch_integrity);
            assert!(!c.one_at_a_time); // explicit one_at_a_time="false"
            assert!(!c.exponential_leak);
        };
        check(&p);
        check(&roundtrip(&p));
    }

    #[test]
    fn plain_conveyor_defaults() {
        let xml = wrap(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>i</inflow><outflow>o</outflow>
          <conveyor><len>transit</len></conveyor>
        </stock>
        <flow name="i"><eqn>1</eqn></flow>
        <flow name="o"><eqn>0</eqn></flow>"#,
        );
        let p = roundtrip(&parse(&xml));
        let c = find_stock(&p, "belt")
            .compat
            .conveyor
            .as_ref()
            .expect("conveyor");
        assert_eq!(c.transit_time, "transit");
        assert_eq!(c.capacity, None);
        assert!(!c.discrete);
        assert!(c.one_at_a_time, "one_at_a_time defaults to true");
        assert!(!c.exponential_leak);
    }

    #[test]
    fn exponential_leak_attr_roundtrips() {
        let xml = wrap(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>i</inflow><outflow>o</outflow>
          <conveyor exponential_leak="true"><len>4</len></conveyor>
        </stock>
        <flow name="i"><eqn>1</eqn></flow>
        <flow name="o"><eqn>0</eqn></flow>"#,
        );
        let p = roundtrip(&parse(&xml));
        let c = find_stock(&p, "belt").compat.conveyor.as_ref().unwrap();
        assert!(c.exponential_leak);
    }

    #[test]
    fn leak_marker_plus_eqn_encoding() {
        // Stella's form: a bare <leak/> marker; the fraction lives in <eqn>.
        // Leakage.fraction stays None (the runtime falls back to the eqn).
        let xml = wrap(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>i</inflow><outflow>o</outflow><outflow>attriting</outflow>
          <conveyor><len>4</len></conveyor>
        </stock>
        <flow name="i"><eqn>1</eqn></flow>
        <flow name="o"><eqn>0</eqn></flow>
        <flow name="attriting"><eqn>0.1</eqn><non_negative/><leak/></flow>"#,
        );
        let p = parse(&xml);
        let check = |p: &datamodel::Project| {
            let f = find_flow(p, "attriting");
            let l = f.compat.leakage.as_ref().expect("leakage");
            assert_eq!(l.fraction, None, "bare <leak/> leaves fraction in the eqn");
            assert!(!l.integers);
            // the eqn (0.1) survives as the flow equation for round-trip + fallback
            assert!(matches!(&f.equation, datamodel::Equation::Scalar(s) if s == "0.1"));
        };
        check(&p);
        check(&roundtrip(&p));
    }

    #[test]
    fn leak_value_bearing_encoding_and_zone() {
        // The spec's value-bearing form with leak_integers + a zone.
        let xml = wrap(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>i</inflow><outflow>o</outflow><outflow>attriting</outflow>
          <conveyor><len>4</len></conveyor>
        </stock>
        <flow name="i"><eqn>1</eqn></flow>
        <flow name="o"><eqn>0</eqn></flow>
        <flow name="attriting" leak_start="0" leak_end="0.25"><leak>0.1</leak><leak_integers/></flow>"#,
        );
        let p = parse(&xml);
        let check = |p: &datamodel::Project| {
            let l = find_flow(p, "attriting")
                .compat
                .leakage
                .as_ref()
                .expect("leakage");
            assert_eq!(l.fraction.as_deref(), Some("0.1"));
            assert!(l.integers);
            assert_eq!(l.zone_start.as_deref(), Some("0"));
            assert_eq!(l.zone_end.as_deref(), Some("0.25"));
        };
        check(&p);
        check(&roundtrip(&p));
    }

    #[test]
    fn spreadflow_dist_roundtrips() {
        let xml = wrap(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>infecting</inflow><outflow>o</outflow>
          <conveyor><len>4</len></conveyor>
        </stock>
        <flow name="infecting" isee:spreadflow="dist"><eqn>1</eqn><isee:distrib_eq>profile</isee:distrib_eq></flow>
        <flow name="o"><eqn>0</eqn></flow>"#,
        );
        let p = parse(&xml);
        let check = |p: &datamodel::Project| {
            let s = find_flow(p, "infecting")
                .compat
                .spreadflow
                .as_ref()
                .expect("spreadflow");
            assert_eq!(*s, SpreadFlow::Dist("profile".to_string()));
        };
        check(&p);
        check(&roundtrip(&p));
    }

    #[test]
    fn spreadflow_even_and_source_roundtrip() {
        let xml = wrap(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>a</inflow><inflow>b</inflow><outflow>o</outflow>
          <conveyor><len>4</len></conveyor>
        </stock>
        <flow name="a" isee:spreadflow="even"><eqn>1</eqn></flow>
        <flow name="b" isee:spreadflow="source"><eqn>1</eqn></flow>
        <flow name="o"><eqn>0</eqn></flow>"#,
        );
        let p = roundtrip(&parse(&xml));
        assert_eq!(
            *find_flow(&p, "a").compat.spreadflow.as_ref().unwrap(),
            SpreadFlow::Even
        );
        assert_eq!(
            *find_flow(&p, "b").compat.spreadflow.as_ref().unwrap(),
            SpreadFlow::Source
        );
    }

    /// The `test/conveyors/minimal_conveyor.xmile` fixture declares
    /// `<uses_conveyor/>` in its header; a full read -> write -> read must
    /// preserve that declaration (conveyors.md sections 3.1/9.7 -- Stella/isee
    /// interop relies on the header advertising the feature). The fixture's
    /// conveyor has no `<arrest>` and no leak flows, so the re-emitted header
    /// must carry neither advisory attribute (mirroring the bare
    /// `<uses_queue/>` rule for a plain queue).
    #[test]
    fn minimal_conveyor_fixture_header_roundtrips() {
        let xml = include_str!("../../../../test/conveyors/minimal_conveyor.xmile");
        let p = parse(xml);
        let written = project_to_xmile(&p).expect("serialize");
        assert!(
            written.contains("<uses_conveyor/>"),
            "writer must emit a bare <uses_conveyor/> header: {written}"
        );
        assert!(
            !written.contains("arrest="),
            "no conveyor uses arrest, so the header must not announce it: {written}"
        );
        assert!(
            !written.contains("leak="),
            "no flow carries leakage, so the header must not announce it: {written}"
        );
        // The header survives a SECOND generation too (read the writer's own
        // output and write again), so the declaration never decays.
        let p2 = parse(&written);
        let written2 = project_to_xmile(&p2).expect("serialize");
        assert!(
            written2.contains("<uses_conveyor/>"),
            "header lost on second-generation write: {written2}"
        );
    }

    /// A conveyor using `<arrest>` plus a leak-marked outflow must advertise
    /// both sub-features on the header (`arrest="true" leak="true"`,
    /// conveyors.md section 3.1), and the emitted attributes must round-trip
    /// through the actual reader into the parsed [`crate::xmile::Feature`] --
    /// pinning that the writer spells the attributes the way the parser
    /// deserializes them (attribute form, not child elements).
    #[test]
    fn conveyor_header_announces_arrest_and_leak() {
        let xml = wrap(
            r#"
        <stock name="belt"><eqn>10</eqn><inflow>i</inflow>
          <outflow>o</outflow><outflow>l</outflow>
          <conveyor><len>4</len><arrest>0</arrest></conveyor>
        </stock>
        <flow name="i"><eqn>1</eqn></flow>
        <flow name="o"><eqn>0</eqn></flow>
        <flow name="l"><eqn>0.05</eqn><leak/></flow>"#,
        );
        let p = parse(&xml);
        let written = project_to_xmile(&p).expect("serialize");
        assert!(
            written.contains(r#"<uses_conveyor arrest="true" leak="true"/>"#),
            "writer must announce both sub-features: {written}"
        );

        // Round-trip the writer's output through the real reader at the
        // xmile::File level: the datamodel does not carry header features
        // (the per-stock block is authoritative), so parser fidelity is only
        // observable on the deserialized Feature itself.
        let file: crate::xmile::File =
            quick_xml::de::from_reader(written.as_bytes()).expect("reparse written XML");
        let features = file
            .header
            .expect("header")
            .options
            .expect("options")
            .features
            .expect("features");
        assert!(
            features.iter().any(|f| matches!(
                f,
                crate::xmile::Feature::UsesConveyor {
                    arrest: Some(true),
                    leak: Some(true),
                }
            )),
            "parser must read back the arrest/leak attributes the writer emitted",
        );
    }

    /// A leak flow alone (no `<arrest>`) announces only `leak="true"`; the
    /// two advisory attributes are independent.
    #[test]
    fn conveyor_header_leak_only() {
        let xml = wrap(
            r#"
        <stock name="belt"><eqn>10</eqn>
          <outflow>o</outflow><outflow>l</outflow>
          <conveyor><len>4</len></conveyor>
        </stock>
        <flow name="o"><eqn>0</eqn></flow>
        <flow name="l"><eqn>0.05</eqn><leak/></flow>"#,
        );
        let p = parse(&xml);
        let written = project_to_xmile(&p).expect("serialize");
        assert!(
            written.contains(r#"<uses_conveyor leak="true"/>"#),
            "expected leak-only header: {written}"
        );
        assert!(
            !written.contains("arrest="),
            "no conveyor uses arrest: {written}"
        );
    }

    #[test]
    fn minimal_conveyor_fixture_parses() {
        let xml = include_str!("../../../../test/conveyors/minimal_conveyor.xmile");
        let p = parse(xml);
        let c = find_stock(&p, "Students")
            .compat
            .conveyor
            .as_ref()
            .expect("conveyor");
        assert_eq!(c.transit_time, "4");
        assert_eq!(c.capacity.as_deref(), Some("1200"));
        // full round-trip preserves it
        let c2ref = roundtrip(&p);
        let c2 = find_stock(&c2ref, "Students")
            .compat
            .conveyor
            .as_ref()
            .unwrap();
        assert_eq!(c2.transit_time, "4");
        assert_eq!(c2.capacity.as_deref(), Some("1200"));
    }
}

#[cfg(test)]
mod queue_tests {
    use crate::datamodel;
    use crate::xmile::{project_from_reader, project_to_xmile};
    use std::io::BufReader;

    fn parse(xml: &str) -> datamodel::Project {
        project_from_reader(&mut BufReader::new(xml.as_bytes())).expect("parse")
    }

    fn roundtrip(project: &datamodel::Project) -> (String, datamodel::Project) {
        let xml = project_to_xmile(project).expect("serialize");
        let reparsed = parse(&xml);
        (xml, reparsed)
    }

    fn find_stock<'a>(p: &'a datamodel::Project, name: &str) -> &'a datamodel::Stock {
        p.models[0]
            .variables
            .iter()
            .find_map(|v| match v {
                datamodel::Variable::Stock(s) if s.ident == name => Some(s),
                _ => None,
            })
            .unwrap_or_else(|| panic!("stock {name} not found"))
    }

    fn find_flow<'a>(p: &'a datamodel::Project, name: &str) -> &'a datamodel::Flow {
        p.models[0]
            .variables
            .iter()
            .find_map(|v| match v {
                datamodel::Variable::Flow(f) if f.ident == name => Some(f),
                _ => None,
            })
            .unwrap_or_else(|| panic!("flow {name} not found"))
    }

    /// The `test/queues/minimal_queue.xmile` fixture (a queue stock with a
    /// primary + an `<overflow/>` outflow, plus `<uses_queue overflow="true"/>`)
    /// survives read -> write -> read with the queue marker, the overflow
    /// marker, AND the header preserved -- no data loss (queues.md §11 step 1).
    #[test]
    fn minimal_queue_fixture_roundtrips_without_loss() {
        let xml = include_str!("../../../../test/queues/minimal_queue.xmile");
        let p = parse(xml);

        // The initial parse recognizes the queue block and the overflow marker.
        assert!(
            find_stock(&p, "waiting").compat.queue.is_some(),
            "the <queue/> block must set compat.queue"
        );
        assert!(
            find_flow(&p, "balk").compat.overflow,
            "the <overflow/> marker must set compat.overflow"
        );
        assert!(
            !find_flow(&p, "into_service").compat.overflow,
            "the primary outflow is not an overflow"
        );

        // Read -> write -> read preserves the markers.
        let (written, p2) = roundtrip(&p);
        assert!(
            find_stock(&p2, "waiting").compat.queue.is_some(),
            "queue marker lost on round-trip"
        );
        assert!(
            find_flow(&p2, "balk").compat.overflow,
            "overflow marker lost on round-trip"
        );

        // The writer re-emits the block-level markers and the header.
        assert!(
            written.contains("<queue"),
            "writer must emit <queue/>: {written}"
        );
        assert!(
            written.contains("<overflow"),
            "writer must emit <overflow/>: {written}"
        );
        assert!(
            written.contains(r#"<uses_queue overflow="true"/>"#),
            "writer must emit the <uses_queue overflow=\"true\"/> header: {written}"
        );

        // Parser fidelity: the overflow attribute the writer emits must land
        // in the deserialized Feature (attribute form, not a child element) --
        // the same writer/reader agreement the conveyor twin pins.
        let file: crate::xmile::File =
            quick_xml::de::from_reader(written.as_bytes()).expect("reparse written XML");
        let features = file
            .header
            .expect("header")
            .options
            .expect("options")
            .features
            .expect("features");
        assert!(
            features.iter().any(|f| matches!(
                f,
                crate::xmile::Feature::UsesQueue {
                    overflow: Some(true),
                }
            )),
            "parser must read back the overflow attribute the writer emitted",
        );
    }

    /// A stock carrying BOTH a `<conveyor>` block and a `<queue/>` marker is a
    /// type conflict the COMPILER rejects (F12,
    /// [`crate::common::ErrorCode::StockBothConveyorAndQueue`]), but the READER
    /// must stay faithful: it parses both markers onto the stock and the writer
    /// re-emits both. Rejection is deliberately a compile-time concern, not a
    /// parse-time one, so the reader never silently drops one marker (which would
    /// mask the conflict and turn an invalid model into a plausible-looking one).
    /// This pins that layering: both markers survive read -> write -> read.
    #[test]
    fn both_conveyor_and_queue_markers_roundtrip_faithfully() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>1</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="belt"><eqn>10</eqn><outflow>out</outflow>
      <conveyor><len>4</len></conveyor><queue/></stock>
    <flow name="out"><eqn>0</eqn></flow>
  </variables></model>
</xmile>"#;
        let p = parse(xml);
        // The reader sets BOTH fields independently -- neither shadows the other.
        assert!(
            find_stock(&p, "belt").compat.conveyor.is_some(),
            "the <conveyor> block must set compat.conveyor"
        );
        assert!(
            find_stock(&p, "belt").compat.queue.is_some(),
            "the <queue/> marker must set compat.queue"
        );

        // Read -> write -> read preserves both markers (the writer emits both).
        let (written, p2) = roundtrip(&p);
        assert!(
            written.contains("<conveyor"),
            "writer must emit <conveyor>: {written}"
        );
        assert!(
            written.contains("<queue"),
            "writer must emit <queue/>: {written}"
        );
        assert!(
            find_stock(&p2, "belt").compat.conveyor.is_some(),
            "conveyor marker lost on round-trip"
        );
        assert!(
            find_stock(&p2, "belt").compat.queue.is_some(),
            "queue marker lost on round-trip"
        );
    }

    /// A plain queue (no overflow outflow) emits a bare `<uses_queue/>` header,
    /// never `overflow="true"`.
    #[test]
    fn plain_queue_header_has_no_overflow_attr() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>1</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="q"><eqn>0</eqn><outflow>o</outflow><queue/></stock>
    <flow name="o"><eqn>0</eqn></flow>
  </variables></model>
</xmile>"#;
        let p = parse(xml);
        let written = project_to_xmile(&p).expect("serialize");
        assert!(
            written.contains("<uses_queue/>"),
            "expected a bare <uses_queue/>: {written}"
        );
        assert!(
            !written.contains("overflow"),
            "a queue with no overflow outflow must not announce overflow: {written}"
        );
    }
}

#[cfg(test)]
mod gf_element_tests {
    //! Round-trip tests for gf-only `<element>` blocks (GH #907): an
    //! `<element>` may legally carry just a `<gf>` (XMILE 4.5.2). The reader
    //! maps the absent per-element `<eqn>` to an empty element equation; the
    //! writer must re-emit such elements WITHOUT a spurious empty `<eqn>` so
    //! the round-trip is stable at both the datamodel and the XML level.

    use crate::datamodel;
    use crate::xmile::{project_from_reader, project_to_xmile};
    use std::io::BufReader;

    /// Wrap variable XML in a minimal, valid XMILE project with one dimension.
    fn wrap(vars: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months">
    <start>0</start><stop>12</stop><dt>0.25</dt>
  </sim_specs>
  <dimensions>
    <dim name="Product"><elem name="Pizza"/><elem name="Salad"/></dim>
  </dimensions>
  <model><variables>{vars}</variables></model>
</xmile>"#
        )
    }

    fn parse(xml: &str) -> datamodel::Project {
        project_from_reader(&mut BufReader::new(xml.as_bytes())).expect("parse")
    }

    fn find_aux<'a>(p: &'a datamodel::Project, name: &str) -> &'a datamodel::Aux {
        p.models[0]
            .variables
            .iter()
            .find_map(|v| match v {
                datamodel::Variable::Aux(a) if a.ident == name => Some(a),
                _ => None,
            })
            .unwrap_or_else(|| panic!("aux {name} not found"))
    }

    /// A pure per-element table holder: gf-only elements, no top-level eqn.
    const GF_ONLY_AUX: &str = r#"
        <aux name="tables">
          <dimensions><dim name="Product"/></dimensions>
          <element subscript="Pizza">
            <gf><xscale min="0" max="1"/><ypts>0,10</ypts></gf>
          </element>
          <element subscript="Salad">
            <gf><xscale min="0" max="1"/><ypts>0,20</ypts></gf>
          </element>
        </aux>"#;

    #[test]
    fn gf_only_elements_parse_to_empty_equations() {
        let p = parse(&wrap(GF_ONLY_AUX));
        let aux = find_aux(&p, "tables");
        match &aux.equation {
            datamodel::Equation::Arrayed(dims, elements, default_eq, has_except) => {
                assert_eq!(&vec!["product".to_string()], dims);
                assert_eq!(&None, default_eq);
                assert!(!*has_except);
                assert_eq!(2, elements.len());
                for (subscript, eqn, initial_eqn, gf) in elements {
                    assert!(
                        eqn.is_empty(),
                        "gf-only element {subscript} must get an empty equation"
                    );
                    assert_eq!(&None, initial_eqn);
                    assert!(gf.is_some(), "element {subscript} must keep its gf");
                }
            }
            _ => panic!("expected an Arrayed equation"),
        }
    }

    #[test]
    fn gf_only_elements_write_without_empty_eqn_tag() {
        let p = parse(&wrap(GF_ONLY_AUX));
        let written = project_to_xmile(&p).expect("serialize");
        assert!(
            !written.contains("<eqn></eqn>") && !written.contains("<eqn/>"),
            "the writer must re-emit a gf-only element with NO <eqn> tag, got: {written}"
        );
        // and the round-trip is stable at the datamodel level
        let p2 = parse(&written);
        assert_eq!(
            find_aux(&p, "tables").equation,
            find_aux(&p2, "tables").equation
        );
    }

    /// The non-a2a-gf.stmx shape: a top-level `<eqn>` coexisting with gf-only
    /// elements. The top-level eqn is the EXCEPT default (applied to every
    /// element, since none carries its own equation) and must survive the
    /// round-trip; the per-element gfs stay attached to their elements.
    #[test]
    fn gf_only_elements_with_top_level_eqn_keep_except_default() {
        let aux_xml = r#"
        <aux name="c">
          <dimensions><dim name="Product"/></dimensions>
          <element subscript="Pizza">
            <gf><xscale min="0" max="1"/><ypts>0,10</ypts></gf>
          </element>
          <element subscript="Salad">
            <gf><xscale min="0" max="1"/><ypts>0,20</ypts></gf>
          </element>
          <eqn>TIME</eqn>
        </aux>"#;
        let p = parse(&wrap(aux_xml));
        let check = |p: &datamodel::Project| {
            let aux = find_aux(p, "c");
            match &aux.equation {
                datamodel::Equation::Arrayed(_, elements, default_eq, has_except) => {
                    assert_eq!(Some("TIME"), default_eq.as_deref());
                    assert!(*has_except);
                    assert_eq!(2, elements.len());
                    for (subscript, eqn, _, gf) in elements {
                        assert!(eqn.is_empty(), "element {subscript} has no eqn of its own");
                        assert!(gf.is_some());
                    }
                }
                _ => panic!("expected an Arrayed equation"),
            }
        };
        check(&p);
        let written = project_to_xmile(&p).expect("serialize");
        check(&parse(&written));
    }
}
