// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::datamodel::view_element;
use crate::diagram::common::{
    Rect, display_name, escape_xml_attr, escape_xml_text, js_format_number,
};
use crate::diagram::constants::*;
use crate::diagram::label::{LabelProps, element_with_label_bounds, render_label};

const CLOUD_PATH: &str = "M 25.731189,3.8741489 C 21.525742,3.8741489 18.07553,7.4486396 17.497605,12.06118 C 16.385384,10.910965 14.996889,10.217536 13.45908,10.217535 C 9.8781481,10.217535 6.9473481,13.959873 6.9473482,18.560807 C 6.9473482,19.228828 7.0507906,19.875499 7.166493,20.498196 C 3.850265,21.890233 1.5000346,25.3185 1.5000346,29.310191 C 1.5000346,34.243794 5.1009986,38.27659 9.6710049,38.715902 C 9.6186538,39.029349 9.6083922,39.33212 9.6083922,39.653348 C 9.6083922,45.134228 17.378069,49.59028 26.983444,49.590279 C 36.58882,49.590279 44.389805,45.134229 44.389803,39.653348 C 44.389803,39.35324 44.341646,39.071755 44.295883,38.778399 C 44.369863,38.780301 44.440617,38.778399 44.515029,38.778399 C 49.470875,38.778399 53.499966,34.536825 53.499965,29.310191 C 53.499965,24.377592 49.928977,20.313927 45.360301,19.873232 C 45.432415,19.39158 45.485527,18.91118 45.485527,18.404567 C 45.485527,13.821862 42.394553,10.092543 38.598118,10.092543 C 36.825927,10.092543 35.215888,10.918252 33.996078,12.248669 C 33.491655,7.5434856 29.994502,3.8741489 25.731189,3.8741489 z";

// --- Auxiliary ---

pub fn render_aux(element: &view_element::Aux, is_arrayed: bool) -> String {
    let cx = element.x;
    let cy = element.y;
    let r = AUX_RADIUS;
    let arrayed_offset = if is_arrayed { ARRAYED_OFFSET } else { 0.0 };

    let label_props = LabelProps::new(cx, cy, element.label_side, display_name(&element.name))
        .with_radii(r + arrayed_offset, r + arrayed_offset);

    let mut svg = String::new();
    svg.push_str("<g class=\"simlin-aux\">");

    if is_arrayed {
        for offset in [arrayed_offset, 0.0, -arrayed_offset] {
            svg.push_str(&format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\"></circle>",
                js_format_number(cx + offset),
                js_format_number(cy + offset),
                js_format_number(r)
            ));
        }
    } else {
        svg.push_str(&format!(
            "<circle cx=\"{}\" cy=\"{}\" r=\"{}\"></circle>",
            js_format_number(cx),
            js_format_number(cy),
            js_format_number(r)
        ));
    }

    // TODO(sparklines): render sparkline here when simulation results are available
    svg.push_str(&render_label(&label_props));
    svg.push_str("</g>");
    svg
}

pub fn aux_bounds(element: &view_element::Aux) -> Rect {
    let cx = element.x;
    let cy = element.y;
    let r = AUX_RADIUS;
    let bounds = Rect {
        top: cy - r,
        left: cx - r,
        right: cx + r,
        bottom: cy + r,
    };

    let label_props = LabelProps::new(cx, cy, element.label_side, display_name(&element.name));
    element_with_label_bounds(bounds, &label_props)
}

// --- Stock ---

pub fn render_stock(element: &view_element::Stock, is_arrayed: bool) -> String {
    let cx = element.x;
    let cy = element.y;
    let w = STOCK_WIDTH;
    let h = STOCK_HEIGHT;
    let arrayed_offset = if is_arrayed { ARRAYED_OFFSET } else { 0.0 };

    let x = cx - w / 2.0;
    let y = cy - h / 2.0;

    let label_props = LabelProps::new(cx, cy, element.label_side, display_name(&element.name))
        .with_radii(w / 2.0 + arrayed_offset, h / 2.0 + arrayed_offset);

    let mut svg = String::new();
    svg.push_str("<g class=\"simlin-stock\">");

    if is_arrayed {
        for offset in [arrayed_offset, 0.0, -arrayed_offset] {
            svg.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"></rect>",
                js_format_number(x + offset),
                js_format_number(y + offset),
                js_format_number(w),
                js_format_number(h)
            ));
        }
    } else {
        svg.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"></rect>",
            js_format_number(x),
            js_format_number(y),
            js_format_number(w),
            js_format_number(h)
        ));
    }

    // TODO(sparklines): render sparkline here when simulation results are available
    svg.push_str(&render_label(&label_props));
    svg.push_str("</g>");
    svg
}

pub fn stock_bounds(element: &view_element::Stock) -> Rect {
    let cx = element.x;
    let cy = element.y;
    let w = STOCK_WIDTH;
    let h = STOCK_HEIGHT;
    let bounds = Rect {
        top: cy - h / 2.0,
        left: cx - w / 2.0,
        right: cx + w / 2.0,
        bottom: cy + h / 2.0,
    };

    let label_props = LabelProps::new(cx, cy, element.label_side, display_name(&element.name))
        .with_radii(w / 2.0, h / 2.0);
    element_with_label_bounds(bounds, &label_props)
}

// --- Module ---

pub fn render_module(element: &view_element::Module) -> String {
    let cx = element.x;
    let cy = element.y;
    let w = MODULE_WIDTH;
    let h = MODULE_HEIGHT;

    // TS uses Math.ceil for x and y
    let x = (cx - w / 2.0).ceil();
    let y = (cy - h / 2.0).ceil();

    let label_props = LabelProps::new(cx, cy, element.label_side, display_name(&element.name))
        .with_radii(w / 2.0, h / 2.0);

    let mut svg = String::new();
    svg.push_str("<g class=\"simlin-module\">");
    svg.push_str(&format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" ry=\"{}\"></rect>",
        js_format_number(x),
        js_format_number(y),
        js_format_number(w),
        js_format_number(h),
        js_format_number(MODULE_RADIUS),
        js_format_number(MODULE_RADIUS)
    ));
    svg.push_str(&render_label(&label_props));
    svg.push_str("</g>");
    svg
}

pub fn module_bounds(element: &view_element::Module) -> Rect {
    let cx = element.x;
    let cy = element.y;
    let w = MODULE_WIDTH;
    let h = MODULE_HEIGHT;
    Rect {
        top: cy - h / 2.0,
        left: cx - w / 2.0,
        right: cx + w / 2.0,
        bottom: cy + h / 2.0,
    }
}

// --- Cloud ---

pub fn render_cloud(element: &view_element::Cloud) -> String {
    let x = element.x;
    let y = element.y;
    let radius = CLOUD_RADIUS;
    let diameter = radius * 2.0;
    let scale = diameter / CLOUD_WIDTH;

    let transform = format!(
        "matrix({}, 0, 0, {}, {}, {})",
        js_format_number(scale),
        js_format_number(scale),
        js_format_number(x - radius),
        js_format_number(y - radius)
    );

    format!(
        "<path d=\"{}\" class=\"simlin-cloud\" transform=\"{}\"></path>",
        escape_xml_attr(CLOUD_PATH),
        escape_xml_attr(&transform)
    )
}

pub fn cloud_bounds(element: &view_element::Cloud) -> Rect {
    let x = element.x;
    let y = element.y;
    let radius = CLOUD_RADIUS;
    Rect {
        top: y - radius,
        left: x - radius,
        right: x + radius,
        bottom: y + radius,
    }
}

// --- Alias ---

pub fn render_alias(element: &view_element::Alias, alias_of_name: Option<&str>) -> String {
    let cx = element.x;
    let cy = element.y;
    let r = AUX_RADIUS;
    let name = alias_of_name.unwrap_or("unknown alias");

    // Alias hardcodes isArrayed = false
    let label_props =
        LabelProps::new(cx, cy, element.label_side, display_name(name)).with_radii(r, r);

    let mut svg = String::new();
    svg.push_str("<g class=\"simlin-alias\">");
    svg.push_str(&format!(
        "<circle cx=\"{}\" cy=\"{}\" r=\"{}\"></circle>",
        js_format_number(cx),
        js_format_number(cy),
        js_format_number(r)
    ));
    // TODO(sparklines): render sparkline here when simulation results are available
    svg.push_str(&render_label(&label_props));
    svg.push_str("</g>");
    svg
}

// --- Group ---

pub fn render_group(element: &view_element::Group) -> String {
    let x = element.x;
    let y = element.y;
    let width = element.width;
    let height = element.height;
    let name = &element.name;

    let left = x - width / 2.0;
    let top = y - height / 2.0;

    let mut svg = String::new();
    svg.push_str("<g class=\"simlin-group\">");
    svg.push_str(&format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" ry=\"{}\"></rect>",
        js_format_number(left),
        js_format_number(top),
        js_format_number(width),
        js_format_number(height),
        js_format_number(GROUP_RADIUS),
        js_format_number(GROUP_RADIUS)
    ));
    svg.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" dominant-baseline=\"hanging\">",
        js_format_number(left + GROUP_LABEL_PADDING),
        js_format_number(top + GROUP_LABEL_PADDING)
    ));
    svg.push_str(&escape_xml_text(&display_name(name)));
    svg.push_str("</text>");
    svg.push_str("</g>");
    svg
}

pub fn group_bounds(element: &view_element::Group) -> Rect {
    let x = element.x;
    let y = element.y;
    let width = element.width;
    let height = element.height;
    let left = x - width / 2.0;
    let top = y - height / 2.0;
    Rect {
        top,
        left,
        right: left + width,
        bottom: top + height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamodel::view_element::LabelSide;

    fn make_aux(x: f64, y: f64, name: &str) -> view_element::Aux {
        view_element::Aux {
            name: name.to_string(),
            uid: 1,
            x,
            y,
            label_side: LabelSide::Bottom,
        }
    }

    fn make_stock(x: f64, y: f64, name: &str) -> view_element::Stock {
        view_element::Stock {
            name: name.to_string(),
            uid: 2,
            x,
            y,
            label_side: LabelSide::Bottom,
        }
    }

    #[test]
    fn test_render_aux_basic() {
        let element = make_aux(100.0, 200.0, "population");
        let svg = render_aux(&element, false);
        assert!(svg.starts_with("<g class=\"simlin-aux\">"));
        assert!(svg.contains("<circle cx=\"100\" cy=\"200\" r=\"9\"></circle>"));
        assert!(svg.contains("population"));
        assert!(svg.ends_with("</g>"));
    }

    #[test]
    fn test_render_aux_arrayed() {
        let element = make_aux(100.0, 200.0, "population");
        let svg = render_aux(&element, true);
        // Should have 3 circles for arrayed
        let circle_count = svg.matches("<circle").count();
        assert_eq!(circle_count, 3);
    }

    #[test]
    fn test_aux_bounds() {
        let element = make_aux(100.0, 200.0, "test");
        let bounds = aux_bounds(&element);
        assert!(bounds.left <= 100.0 - AUX_RADIUS);
        assert!(bounds.right >= 100.0 + AUX_RADIUS);
    }

    #[test]
    fn test_render_stock_basic() {
        let element = make_stock(150.0, 250.0, "inventory");
        let svg = render_stock(&element, false);
        assert!(svg.starts_with("<g class=\"simlin-stock\">"));
        assert!(svg.contains("<rect"));
        assert!(svg.contains("inventory"));
    }

    #[test]
    fn test_render_stock_arrayed() {
        let element = make_stock(150.0, 250.0, "inventory");
        let svg = render_stock(&element, true);
        let rect_count = svg.matches("<rect").count();
        assert_eq!(rect_count, 3);
    }

    #[test]
    fn test_stock_bounds() {
        let element = make_stock(150.0, 250.0, "test");
        let bounds = stock_bounds(&element);
        assert_eq!(bounds.left, 150.0 - STOCK_WIDTH / 2.0);
        assert!(bounds.top <= 250.0 - STOCK_HEIGHT / 2.0);
    }

    #[test]
    fn test_render_module() {
        let element = view_element::Module {
            name: "submodel".to_string(),
            uid: 3,
            x: 200.0,
            y: 300.0,
            label_side: LabelSide::Bottom,
        };
        let svg = render_module(&element);
        assert!(svg.contains("simlin-module"));
        assert!(svg.contains("rx=\"5\""));
        assert!(svg.contains("ry=\"5\""));
        assert!(svg.contains("submodel"));
    }

    #[test]
    fn test_render_cloud() {
        let element = view_element::Cloud {
            uid: 4,
            flow_uid: 5,
            x: 100.0,
            y: 200.0,
        };
        let svg = render_cloud(&element);
        assert!(svg.contains("simlin-cloud"));
        assert!(svg.contains("matrix("));
        assert!(svg.contains("<path"));
    }

    #[test]
    fn test_cloud_bounds() {
        let element = view_element::Cloud {
            uid: 4,
            flow_uid: 5,
            x: 100.0,
            y: 200.0,
        };
        let bounds = cloud_bounds(&element);
        assert_eq!(bounds.left, 100.0 - CLOUD_RADIUS);
        assert_eq!(bounds.right, 100.0 + CLOUD_RADIUS);
    }

    #[test]
    fn test_render_alias() {
        let element = view_element::Alias {
            uid: 6,
            alias_of_uid: 1,
            x: 100.0,
            y: 200.0,
            label_side: LabelSide::Bottom,
        };
        let svg = render_alias(&element, Some("population"));
        assert!(svg.contains("simlin-alias"));
        assert!(svg.contains("population"));
    }

    #[test]
    fn test_render_alias_unknown() {
        let element = view_element::Alias {
            uid: 6,
            alias_of_uid: 1,
            x: 100.0,
            y: 200.0,
            label_side: LabelSide::Bottom,
        };
        let svg = render_alias(&element, None);
        assert!(svg.contains("unknown alias"));
    }

    #[test]
    fn test_render_group() {
        let element = view_element::Group {
            uid: 7,
            name: "my_group".to_string(),
            x: 200.0,
            y: 200.0,
            width: 300.0,
            height: 200.0,
        };
        let svg = render_group(&element);
        assert!(svg.contains("simlin-group"));
        assert!(svg.contains("dominant-baseline=\"hanging\""));
        assert!(svg.contains("my group")); // display_name converts _ to space
    }

    #[test]
    fn test_group_bounds() {
        let element = view_element::Group {
            uid: 7,
            name: "test".to_string(),
            x: 200.0,
            y: 200.0,
            width: 300.0,
            height: 200.0,
        };
        let bounds = group_bounds(&element);
        assert_eq!(bounds.left, 50.0);
        assert_eq!(bounds.top, 100.0);
        assert_eq!(bounds.right, 350.0);
        assert_eq!(bounds.bottom, 300.0);
    }
}
