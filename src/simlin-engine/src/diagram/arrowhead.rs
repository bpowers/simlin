// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::diagram::common::{Point, escape_xml_attr, js_format_number};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ArrowheadType {
    Flow,
    Connector,
}

/// The SVG large-arc flag of the arc that bows an arrowhead's back edge.
pub(crate) const ARROWHEAD_BACK_LARGE_ARC: bool = false;
/// The SVG sweep flag of the arc that bows an arrowhead's back edge.
pub(crate) const ARROWHEAD_BACK_SWEEP: bool = true;

/// An arrowhead's outline before its rotation: a point at `tip`, and a back
/// edge from `back_start` to `back_end` bowed by an arc of radius
/// `back_radius`, drawn with [`ARROWHEAD_BACK_LARGE_ARC`] and
/// [`ARROWHEAD_BACK_SWEEP`]. The drawing turns the whole outline
/// `rotation_deg` degrees about the tip, as SVG `rotate(deg, tip)` does.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct ArrowheadGeometry {
    pub tip: Point,
    pub back_start: Point,
    pub back_end: Point,
    pub back_radius: f64,
    /// The arrowhead's size, which also sizes the SVG-only hit area.
    pub size: f64,
    pub rotation_deg: f64,
}

/// The arrowhead of `size` drawn at `(x, y)` pointing `angle` degrees.
pub(crate) fn arrowhead_geometry(x: f64, y: f64, angle: f64, size: f64) -> ArrowheadGeometry {
    let r = size;
    ArrowheadGeometry {
        tip: Point { x, y },
        back_start: Point {
            x: x - r,
            y: y + r / 2.0,
        },
        back_end: Point {
            x: x - r,
            y: y - r / 2.0,
        },
        back_radius: r * 3.0,
        size,
        rotation_deg: angle,
    }
}

pub(crate) fn render_arrowhead_geometry(g: &ArrowheadGeometry, typ: ArrowheadType) -> String {
    let path = format!(
        "M{},{}L{},{}A{},{} 0 {},{} {},{}z",
        js_format_number(g.tip.x),
        js_format_number(g.tip.y),
        js_format_number(g.back_start.x),
        js_format_number(g.back_start.y),
        js_format_number(g.back_radius),
        js_format_number(g.back_radius),
        ARROWHEAD_BACK_LARGE_ARC as u8,
        ARROWHEAD_BACK_SWEEP as u8,
        js_format_number(g.back_end.x),
        js_format_number(g.back_end.y)
    );

    // The invisible hit area: an SVG pointer target only, never drawn, so a
    // display list does not carry it.
    let (x, y) = (g.tip.x, g.tip.y);
    let bg_r = g.size * 1.5;
    let bg_path = format!(
        "M{},{}L{},{}A{},{} 0 0,1 {},{}z",
        js_format_number(x + 0.5 * bg_r),
        js_format_number(y),
        js_format_number(x - 0.75 * bg_r),
        js_format_number(y + bg_r / 2.0),
        js_format_number(bg_r * 3.0),
        js_format_number(bg_r * 3.0),
        js_format_number(x - 0.75 * bg_r),
        js_format_number(y - bg_r / 2.0)
    );

    let path_class = match typ {
        ArrowheadType::Flow => "simlin-arrowhead-flow",
        ArrowheadType::Connector => "simlin-arrowhead-link",
    };

    let transform = format!(
        "rotate({},{},{})",
        js_format_number(g.rotation_deg),
        js_format_number(x),
        js_format_number(y)
    );

    let mut svg = String::new();
    svg.push_str("<g>");
    svg.push_str(&format!(
        "<path d=\"{}\" class=\"simlin-arrowhead-bg\" transform=\"{}\"></path>",
        escape_xml_attr(&bg_path),
        escape_xml_attr(&transform)
    ));
    svg.push_str(&format!(
        "<path d=\"{}\" class=\"{}\" transform=\"{}\"></path>",
        escape_xml_attr(&path),
        path_class,
        escape_xml_attr(&transform)
    ));
    svg.push_str("</g>");
    svg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_arrowhead_flow() {
        let svg = render_arrowhead_geometry(
            &arrowhead_geometry(100.0, 200.0, 0.0, 8.0),
            ArrowheadType::Flow,
        );
        assert!(svg.contains("simlin-arrowhead-flow"));
        assert!(svg.contains("simlin-arrowhead-bg"));
        assert!(svg.contains("rotate(0,100,200)"));
        assert!(svg.starts_with("<g>"));
        assert!(svg.ends_with("</g>"));
    }

    #[test]
    fn test_render_arrowhead_connector() {
        let svg = render_arrowhead_geometry(
            &arrowhead_geometry(50.0, 60.0, 90.0, 6.0),
            ArrowheadType::Connector,
        );
        assert!(svg.contains("simlin-arrowhead-link"));
        assert!(svg.contains("rotate(90,50,60)"));
    }

    #[test]
    fn test_render_arrowhead_path_structure() {
        let svg = render_arrowhead_geometry(
            &arrowhead_geometry(10.0, 20.0, 0.0, 6.0),
            ArrowheadType::Flow,
        );
        // Main path starts at x,y and creates an arrowhead shape
        assert!(svg.contains("M10,20L"));
        // bg path has the larger radius
        assert!(svg.contains(&format!("M{}", js_format_number(10.0 + 0.5 * 9.0))));
    }

    #[test]
    fn the_printed_arrowhead_is_its_geometry() {
        let g = arrowhead_geometry(10.0, 20.0, 45.0, 6.0);
        assert_eq!(g.tip, Point { x: 10.0, y: 20.0 });
        assert_eq!(g.back_start, Point { x: 4.0, y: 23.0 });
        assert_eq!(g.back_end, Point { x: 4.0, y: 17.0 });
        assert_eq!(g.back_radius, 18.0);
        let svg = render_arrowhead_geometry(&g, ArrowheadType::Connector);
        assert!(svg.contains("d=\"M10,20L4,23A18,18 0 0,1 4,17z\""));
        assert!(svg.contains("rotate(45,10,20)"));
    }
}
