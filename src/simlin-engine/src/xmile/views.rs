// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use float_cmp::approx_eq;
use quick_xml::Writer;
use serde::{Deserialize, Serialize};

use crate::common::{Result, canonicalize};
use crate::datamodel;
use crate::datamodel::{Rect, ViewElement};
use crate::xmile::model::Model;
use crate::xmile::variables::Var;
use crate::xmile::view_element::LinkEnd;
use crate::xmile::{
    STOCK_HEIGHT, STOCK_WIDTH, ToXml, XmlWriter, write_tag_end, write_tag_start_with_attrs,
};

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Copy, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewType {
    StockFlow,
    Interface,
    Popup,
    VendorSpecific,
}

impl ViewType {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            ViewType::StockFlow => "stock_flow",
            ViewType::Interface => "interface",
            ViewType::Popup => "popup",
            ViewType::VendorSpecific => "vendor_specific",
        }
    }
}

pub mod view_element {
    use super::super::datamodel;
    use crate::common::Result;
    #[cfg(test)]
    use crate::datamodel::StockFlow;
    use crate::datamodel::view_element::LinkShape;
    use crate::xmile::{
        STOCK_HEIGHT, STOCK_WIDTH, ToXml, XmlWriter, write_tag, write_tag_end, write_tag_start,
        write_tag_start_with_attrs, write_tag_text, write_tag_with_attrs,
    };
    use quick_xml::Writer;
    use serde::{Deserialize, Deserializer, Serialize};

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

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Copy, Clone, PartialEq, Eq, Deserialize, Serialize)]
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

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone, PartialEq, Deserialize, Serialize)]
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

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone, PartialEq, Deserialize, Serialize)]
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

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone, PartialEq, Deserialize, Serialize)]
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

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone, PartialEq, Default, Deserialize, Serialize)]
    pub struct Points {
        #[serde(rename = "pt")]
        pub points: Vec<Point>,
    }

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone, PartialEq, Deserialize, Serialize)]
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

            if let Some(points) = &self.points
                && !points.points.is_empty()
            {
                write_tag_start(writer, "pts")?;
                for point in &points.points {
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

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
    pub struct AliasLinkEnd {
        #[serde(rename = "@uid")]
        pub uid: i32,
    }

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone, PartialEq, Eq, Serialize)]
    pub enum LinkEnd {
        #[serde(rename = "$value")]
        Named(String),
        #[serde(rename = "alias")]
        Alias(AliasLinkEnd),
    }

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone, PartialEq, Deserialize, Serialize)]
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
        #[serde(rename = "@polarity")]
        pub polarity: Option<String>,
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

            let mut attrs = Vec::with_capacity(2);
            if let Some(ref angle) = angle {
                attrs.push(("angle", angle.as_str()));
            }
            if let Some(ref polarity) = self.polarity {
                attrs.push(("polarity", polarity.as_str()));
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

            if let Some(points) = &self.points
                && !points.points.is_empty()
            {
                write_tag_start(writer, "pts")?;
                for point in &points.points {
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

    fn parse_polarity(s: &str) -> Option<datamodel::view_element::LinkPolarity> {
        match s {
            "+" => Some(datamodel::view_element::LinkPolarity::Positive),
            "-" => Some(datamodel::view_element::LinkPolarity::Negative),
            _ => None,
        }
    }

    fn polarity_to_string(p: &datamodel::view_element::LinkPolarity) -> String {
        match p {
            datamodel::view_element::LinkPolarity::Positive => "+".to_string(),
            datamodel::view_element::LinkPolarity::Negative => "-".to_string(),
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
                polarity: v.polarity.as_deref().and_then(parse_polarity),
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
            polarity: v.polarity.as_deref().and_then(parse_polarity),
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
                polarity: v.polarity.as_ref().map(polarity_to_string),
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
                polarity: None,
            },
            datamodel::view_element::Link {
                uid: 33,
                from_uid: 45,
                to_uid: 67,
                shape: LinkShape::Arc(-45.0), // canvas format
                polarity: None,
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
                polarity: None,
            },
        ];
        let view = StockFlow {
            elements: vec![],
            view_box: Default::default(),
            zoom: 0.0,
            use_lettered_polarity: false,
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
            use_lettered_polarity: false,
        };

        let link = datamodel::view_element::Link {
            uid: 10,
            from_uid: 1,
            to_uid: 2,
            shape: LinkShape::Straight,
            polarity: None,
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
            use_lettered_polarity: false,
        };

        let link = datamodel::view_element::Link {
            uid: 10,
            from_uid: 1,
            to_uid: 2,
            shape: LinkShape::Straight,
            polarity: None,
        };

        let xmile_link = Link::from(link, &view);

        // Canvas angle is 45deg (down-right, Y-down), which converts to XMILE 315deg (Y-up)
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
            use_lettered_polarity: false,
        };

        // Create an XMILE link with angle = 0 (straight horizontal right)
        let xmile_link = Link {
            uid: Some(10),
            from: LinkEnd::Named("from_var".to_string()),
            from_uid: Some(1),
            to: LinkEnd::Named("to_var".to_string()),
            to_uid: Some(2),
            angle: Some(0.0), // canvas coords: 0 degrees = pointing right
            polarity: None,
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
            use_lettered_polarity: false,
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
            polarity: None,
            is_straight: None,
            points: None,
        };

        let dm_link = link_from_xmile_with_view(xmile_link, &view);

        // Should stay as Arc, not Straight
        match dm_link.shape {
            LinkShape::Arc(angle) => {
                // XMILE 45deg converts to canvas -45deg (Y-axis flip)
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
            use_lettered_polarity: false,
        };

        // Angle very close to straight (within epsilon) should become Straight
        let nearly_exact = Link {
            uid: Some(10),
            from: LinkEnd::Named("from_var".to_string()),
            from_uid: Some(1),
            to: LinkEnd::Named("to_var".to_string()),
            to_uid: Some(2),
            angle: Some(0.005), // very close to 0 (straight horizontal)
            polarity: None,
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
            polarity: None,
            is_straight: None,
            points: None,
        };

        let dm_link_off = link_from_xmile_with_view(slightly_off, &view);
        match dm_link_off.shape {
            LinkShape::Arc(_) => {} // expected - preserves original for roundtrip
            _ => panic!("angle not exactly matching should stay as Arc for roundtrip fidelity"),
        }
    }

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone, PartialEq, Deserialize, Serialize)]
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

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone, PartialEq, Deserialize, Serialize)]
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
            use_lettered_polarity: false,
        };
        for expected in cases {
            let expected = expected.clone();
            let actual = datamodel::view_element::Alias::from(Alias::from(expected.clone(), &view));
            assert_eq!(expected, actual);
        }
    }

    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone, PartialEq, Deserialize, Serialize)]
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
    #[cfg_attr(feature = "debug-derive", derive(Debug))]
    #[derive(Clone, PartialEq, Deserialize, Serialize)]
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
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
            ViewObject::Aux(aux) => Some(canonicalize(&aux.name).into_owned()),
            ViewObject::Stock(stock) => Some(canonicalize(&stock.name).into_owned()),
            ViewObject::Flow(flow) => Some(canonicalize(&flow.name).into_owned()),
            ViewObject::Link(_link) => None,
            ViewObject::Module(module) => Some(canonicalize(&module.name).into_owned()),
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
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
                    LinkEnd::Named(name) => uid_map.get(&*canonicalize(name)).cloned(),
                    LinkEnd::Alias(orig_alias) => orig_uid_map.get(&orig_alias.uid).cloned(),
                };
                link.to_uid = match &link.to {
                    LinkEnd::Named(name) => uid_map.get(&*canonicalize(name)).cloned(),
                    LinkEnd::Alias(orig_alias) => orig_uid_map.get(&orig_alias.uid).cloned(),
                };
            } else if let ViewObject::Alias(alias) = o {
                let of_ident = canonicalize(&alias.of);
                alias.of_uid = if !of_ident.is_empty() {
                    uid_map.get(&*of_ident).cloned()
                } else {
                    None
                };
            }
        }

        // if there were links we couldn't resolve, dump them
        let had_unresolvable = self.objects.iter().any(|o| {
            matches!(o, ViewObject::Link(link) if link.from_uid.is_none() || link.to_uid.is_none())
        });
        self.objects.retain(|o| {
            if let ViewObject::Link(link) = o {
                link.from_uid.is_some() && link.to_uid.is_some()
            } else {
                true
            }
        });

        // Re-sequence UIDs to close gaps left by dropped links
        if had_unresolvable {
            uid_map.clear();
            let mut resequence_map: HashMap<i32, i32> = HashMap::new();
            next_uid = 1;
            for o in self.objects.iter_mut() {
                let old_uid = o.uid().unwrap_or(-1);
                resequence_map.insert(old_uid, next_uid);
                if let Some(ident) = o.ident() {
                    uid_map.insert(ident, next_uid);
                }
                o.set_uid(next_uid);
                next_uid += 1;
            }
            let remap = |uid: i32| -> i32 { resequence_map.get(&uid).copied().unwrap_or(uid) };
            for o in self.objects.iter_mut() {
                if let ViewObject::Link(link) = o {
                    link.from_uid = link.from_uid.map(&remap);
                    link.to_uid = link.to_uid.map(&remap);
                } else if let ViewObject::Alias(alias) = o {
                    alias.of_uid = alias.of_uid.map(&remap);
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
            if let Some(Var::Stock(stock)) = model.get_var(&ident) {
                if let Some(outflows) = &stock.outflows {
                    for outflow in outflows {
                        let outflow_ident = canonicalize(outflow).into_owned();
                        if !uid_map.contains_key(&outflow_ident) {
                            continue;
                        }
                        let outflow_uid = uid_map[&outflow_ident];
                        let end = result.get_mut(&outflow_uid).unwrap();
                        end.0 = Some(uid_map[&ident]);
                    }
                }
                if let Some(inflows) = &stock.inflows {
                    for inflow in inflows {
                        let inflow_ident = canonicalize(inflow).into_owned();
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
                if let Some(points) = &mut flow.points
                    && !points.points.is_empty()
                {
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

    pub(crate) fn normalize(&mut self, model: &Model) {
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
            let view_box = if let (Some(x), Some(y), Some(width), Some(height)) =
                (v.offset_x, v.offset_y, v.width, v.height)
            {
                Rect {
                    x,
                    y,
                    width,
                    height,
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
                use_lettered_polarity: false,
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
    use crate::datamodel::Rect;
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
        use_lettered_polarity: false,
    })];
    for expected in cases {
        let expected = expected.clone();
        let actual = datamodel::View::from(View::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}
