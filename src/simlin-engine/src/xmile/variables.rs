// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use quick_xml::Writer;
use serde::{Deserialize, Serialize};

use crate::common::{Result, canonicalize};
use crate::datamodel;
use crate::datamodel::Equation;
use crate::xmile::dimensions::Gf;
use crate::xmile::{
    ToXml, VarDimension, VarDimensions, XmlWriter, write_tag, write_tag_end, write_tag_start,
    write_tag_start_with_attrs,
};

use super::model::{Module, NonNegative, access_from, can_be_module_input, visibility};

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
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
                Some(dimensions) => dimensions.dimensions.unwrap().into_iter().map(|e| canonicalize(&e.name).into_owned()).collect(),
                None => vec![],
            };
            let elements = elements.into_iter().map(|e| {
                let canonical_subscripts: Vec<_> = e.subscript.split(",").map(|s| canonicalize(s.trim()).into_owned()).collect();
                (canonical_subscripts.join(","), e.eqn, e.initial_eqn, e.gf.map(datamodel::GraphicalFunction::from))
            }).collect();
            datamodel::Equation::Arrayed(dimensions, elements)
        } else if let Some(dimensions) = $var.dimensions {
            let dimensions = dimensions.dimensions.unwrap_or_default().into_iter().map(|e| canonicalize(&e.name).into_owned()).collect();
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
                Some(dimensions) => dimensions.dimensions.unwrap().into_iter().map(|e| canonicalize(&e.name).into_owned()).collect(),
                None => vec![],
            };
            let elements = elements.into_iter().map(|e| {
                let canonical_subscripts: Vec<_> = e.subscript.split(",").map(|s| canonicalize(s.trim()).into_owned()).collect();
                (canonical_subscripts.join(","), e.eqn, e.initial_eqn, e.gf.map(datamodel::GraphicalFunction::from))
            }).collect();
            datamodel::Equation::Arrayed(dimensions, elements)
        } else if let Some(dimensions) = $var.dimensions {
            let dimensions = dimensions.dimensions.unwrap_or_default().into_iter().map(|e| canonicalize(&e.name).into_owned()).collect();
            datamodel::Equation::ApplyToAll(dimensions, $var.eqn.unwrap_or_default(), None)
        } else {
            datamodel::Equation::Scalar($var.eqn.unwrap_or_default(), None)
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
        visibility: datamodel::Visibility::Private,
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
