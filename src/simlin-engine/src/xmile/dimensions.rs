// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use quick_xml::Writer;
use serde::{Deserialize, Serialize};

use crate::common::{Result, canonicalize};
use crate::datamodel;
use crate::xmile::{
    ToXml, XmlWriter, write_tag, write_tag_end, write_tag_start_with_attrs, xml_error,
};

use quick_xml::events::{BytesStart, Event};

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
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
        if let Some(size) = self.size {
            let size = format!("{}", size);
            let mut attrs = attrs.clone();
            attrs.push(("size", size.as_str()));
            write_tag_start_with_attrs(writer, "dim", &attrs)?;
        } else {
            write_tag_start_with_attrs(writer, "dim", &attrs)?;
        }

        if let Some(ref elements) = self.elements {
            for element in elements.iter() {
                let attrs = &[("name", element.name.as_str())];
                super::write_tag_with_attrs(writer, "elem", "", attrs)?;
            }
        }

        // Write dimension mapping if present
        if let Some(ref maps_to) = self.maps_to {
            write_tag(writer, "isee:maps_to", maps_to)?;
        }

        super::write_tag_end(writer, "dim")
    }
}

impl From<Dimension> for datamodel::Dimension {
    fn from(dimension: Dimension) -> Self {
        let name = canonicalize(&dimension.name).into_owned();
        let maps_to = dimension.maps_to.map(|m| canonicalize(&m).into_owned());
        let elements = if let Some(elements) = dimension.elements {
            datamodel::DimensionElements::Named(
                elements
                    .into_iter()
                    .map(|i| canonicalize(&i.name).into_owned())
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Index {
    #[serde(rename = "@name")]
    pub name: String,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
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
            super::write_tag_end(writer, "xscale")?;
        }

        if let Some(ref y_scale) = self.y_scale {
            let min = format!("{}", y_scale.min);
            let max = format!("{}", y_scale.max);
            let attrs = &[("min", min.as_str()), ("max", max.as_str())];
            write_tag_start_with_attrs(writer, "yscale", attrs)?;
            super::write_tag_end(writer, "yscale")?;
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
