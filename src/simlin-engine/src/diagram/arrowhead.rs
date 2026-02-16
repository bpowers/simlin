// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::diagram::common::{escape_xml_attr, js_format_number};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ArrowheadType {
    Flow,
    Connector,
}

pub fn render_arrowhead(x: f64, y: f64, angle: f64, size: f64, typ: ArrowheadType) -> String {
    let r = size;
    let path = format!(
        "M{},{}L{},{}A{},{} 0 0,1 {},{}z",
        js_format_number(x),
        js_format_number(y),
        js_format_number(x - r),
        js_format_number(y + r / 2.0),
        js_format_number(r * 3.0),
        js_format_number(r * 3.0),
        js_format_number(x - r),
        js_format_number(y - r / 2.0)
    );

    let bg_r = r * 1.5;
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
        js_format_number(angle),
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
        let svg = render_arrowhead(100.0, 200.0, 0.0, 8.0, ArrowheadType::Flow);
        assert!(svg.contains("simlin-arrowhead-flow"));
        assert!(svg.contains("simlin-arrowhead-bg"));
        assert!(svg.contains("rotate(0,100,200)"));
        assert!(svg.starts_with("<g>"));
        assert!(svg.ends_with("</g>"));
    }

    #[test]
    fn test_render_arrowhead_connector() {
        let svg = render_arrowhead(50.0, 60.0, 90.0, 6.0, ArrowheadType::Connector);
        assert!(svg.contains("simlin-arrowhead-link"));
        assert!(svg.contains("rotate(90,50,60)"));
    }

    #[test]
    fn test_render_arrowhead_path_structure() {
        let svg = render_arrowhead(10.0, 20.0, 0.0, 6.0, ArrowheadType::Flow);
        // Main path starts at x,y and creates an arrowhead shape
        assert!(svg.contains("M10,20L"));
        // bg path has the larger radius
        assert!(svg.contains(&format!("M{}", js_format_number(10.0 + 0.5 * 9.0))));
    }

    #[test]
    fn test_render_arrowhead_180_degrees() {
        let svg = render_arrowhead(100.0, 200.0, 180.0, 8.0, ArrowheadType::Flow);
        assert!(svg.contains("rotate(180,100,200)"));
    }

    #[test]
    fn test_render_arrowhead_270_degrees() {
        let svg = render_arrowhead(100.0, 200.0, 270.0, 8.0, ArrowheadType::Flow);
        assert!(svg.contains("rotate(270,100,200)"));
    }
}
