// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::datamodel::view_element;
use crate::diagram::common::{
    Circle, Frame, Point, Rect, arrayed_offsets, display_name, escape_xml_attr, escape_xml_text,
    js_format_number, svg_circle,
};
use crate::diagram::constants::*;
use crate::diagram::label::{LabelProps, element_with_label_bounds, render_label};

/// The cloud outline, in its own 55-unit design box. Both renderers draw this
/// one path: the SVG as a `matrix` transform over it, the scene with the same
/// [`cloud_transform`] applied to its points.
pub(crate) const CLOUD_PATH: &str = "M 25.731189,3.8741489 C 21.525742,3.8741489 18.07553,7.4486396 17.497605,12.06118 C 16.385384,10.910965 14.996889,10.217536 13.45908,10.217535 C 9.8781481,10.217535 6.9473481,13.959873 6.9473482,18.560807 C 6.9473482,19.228828 7.0507906,19.875499 7.166493,20.498196 C 3.850265,21.890233 1.5000346,25.3185 1.5000346,29.310191 C 1.5000346,34.243794 5.1009986,38.27659 9.6710049,38.715902 C 9.6186538,39.029349 9.6083922,39.33212 9.6083922,39.653348 C 9.6083922,45.134228 17.378069,49.59028 26.983444,49.590279 C 36.58882,49.590279 44.389805,45.134229 44.389803,39.653348 C 44.389803,39.35324 44.341646,39.071755 44.295883,38.778399 C 44.369863,38.780301 44.440617,38.778399 44.515029,38.778399 C 49.470875,38.778399 53.499966,34.536825 53.499965,29.310191 C 53.499965,24.377592 49.928977,20.313927 45.360301,19.873232 C 45.432415,19.39158 45.485527,18.91118 45.485527,18.404567 C 45.485527,13.821862 42.394553,10.092543 38.598118,10.092543 C 36.825927,10.092543 35.215888,10.918252 33.996078,12.248669 C 33.491655,7.5434856 29.994502,3.8741489 25.731189,3.8741489 z";

/// The label an alias whose target is missing from the view shows.
const UNKNOWN_ALIAS_LABEL: &str = "unknown alias";

// --- Auxiliary ---

/// Everything a drawn auxiliary is made of: the SVG prints it and the scene
/// display list reads it.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub(crate) struct AuxGeometry {
    /// The circles, back to front.
    pub circles: Vec<Circle>,
    pub label: LabelProps,
    /// The box the web canvas draws the sparkline in (`Auxiliary.tsx`).
    pub sparkline: Frame,
}

pub(crate) fn aux_geometry(element: &view_element::Aux, is_arrayed: bool) -> AuxGeometry {
    let cx = element.x;
    let cy = element.y;
    let r = AUX_RADIUS;
    let arrayed_offset = if is_arrayed { ARRAYED_OFFSET } else { 0.0 };

    AuxGeometry {
        circles: arrayed_offsets(is_arrayed)
            .iter()
            .map(|offset| Circle {
                x: cx + offset,
                y: cy + offset,
                r,
            })
            .collect(),
        label: LabelProps::new(cx, cy, element.label_side, display_name(&element.name))
            .with_radii(r + arrayed_offset, r + arrayed_offset),
        sparkline: Frame {
            x: cx - arrayed_offset + 1.0 - r / 2.0,
            y: cy - arrayed_offset + 1.0 - r / 2.0,
            width: r - 2.0,
            height: r - 2.0,
        },
    }
}

pub fn render_aux(element: &view_element::Aux, is_arrayed: bool) -> String {
    let g = aux_geometry(element, is_arrayed);

    let mut svg = String::new();
    svg.push_str("<g class=\"simlin-aux\">");
    for circle in &g.circles {
        svg.push_str(&svg_circle(circle));
    }
    svg.push_str(&render_label(&g.label));
    svg.push_str("</g>");
    svg
}

/// The aux's bare *shape* box (the circle's bounding rect), WITHOUT its label.
/// `aux_bounds` is this box merged with the label; quality metrics that already
/// account for labels separately (e.g. label-vs-node overlap) need the
/// label-free shape to avoid double-counting the label area.
pub(crate) fn aux_shape_bounds(element: &view_element::Aux) -> Rect {
    let cx = element.x;
    let cy = element.y;
    let r = AUX_RADIUS;
    Rect {
        top: cy - r,
        left: cx - r,
        right: cx + r,
        bottom: cy + r,
    }
}

pub fn aux_bounds(element: &view_element::Aux) -> Rect {
    let cx = element.x;
    let cy = element.y;
    let bounds = aux_shape_bounds(element);

    let label_props = LabelProps::new(cx, cy, element.label_side, display_name(&element.name));
    element_with_label_bounds(bounds, &label_props)
}

// --- Stock ---

/// Everything a drawn stock is made of.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub(crate) struct StockGeometry {
    /// The rectangles, back to front.
    pub rects: Vec<Frame>,
    pub label: LabelProps,
    /// The box the web canvas draws the sparkline in (`Stock.tsx`): the
    /// rectangle inset by one unit.
    pub sparkline: Frame,
}

pub(crate) fn stock_geometry(element: &view_element::Stock, is_arrayed: bool) -> StockGeometry {
    let cx = element.x;
    let cy = element.y;
    let w = STOCK_WIDTH;
    let h = STOCK_HEIGHT;
    let arrayed_offset = if is_arrayed { ARRAYED_OFFSET } else { 0.0 };

    let x = cx - w / 2.0;
    let y = cy - h / 2.0;

    StockGeometry {
        rects: arrayed_offsets(is_arrayed)
            .iter()
            .map(|offset| Frame {
                x: x + offset,
                y: y + offset,
                width: w,
                height: h,
            })
            .collect(),
        label: LabelProps::new(cx, cy, element.label_side, display_name(&element.name))
            .with_radii(w / 2.0 + arrayed_offset, h / 2.0 + arrayed_offset),
        sparkline: Frame {
            x: cx - arrayed_offset + 1.0 - w / 2.0,
            y: cy - arrayed_offset + 1.0 - h / 2.0,
            width: w - 2.0,
            height: h - 2.0,
        },
    }
}

pub fn render_stock(element: &view_element::Stock, is_arrayed: bool) -> String {
    let g = stock_geometry(element, is_arrayed);

    let mut svg = String::new();
    svg.push_str("<g class=\"simlin-stock\">");
    for rect in &g.rects {
        svg.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"></rect>",
            js_format_number(rect.x),
            js_format_number(rect.y),
            js_format_number(rect.width),
            js_format_number(rect.height)
        ));
    }
    svg.push_str(&render_label(&g.label));
    svg.push_str("</g>");
    svg
}

/// The stock's bare *shape* box (the rect), WITHOUT its label. See
/// `aux_shape_bounds` for why the label-free shape is exposed separately.
pub(crate) fn stock_shape_bounds(element: &view_element::Stock) -> Rect {
    let cx = element.x;
    let cy = element.y;
    let w = STOCK_WIDTH;
    let h = STOCK_HEIGHT;
    Rect {
        top: cy - h / 2.0,
        left: cx - w / 2.0,
        right: cx + w / 2.0,
        bottom: cy + h / 2.0,
    }
}

pub fn stock_bounds(element: &view_element::Stock) -> Rect {
    let cx = element.x;
    let cy = element.y;
    let w = STOCK_WIDTH;
    let h = STOCK_HEIGHT;
    let bounds = stock_shape_bounds(element);

    let label_props = LabelProps::new(cx, cy, element.label_side, display_name(&element.name))
        .with_radii(w / 2.0, h / 2.0);
    element_with_label_bounds(bounds, &label_props)
}

// --- Module ---

/// Everything a drawn module is made of.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub(crate) struct ModuleGeometry {
    pub rect: Frame,
    pub corner_radius: f64,
    pub label: LabelProps,
}

pub(crate) fn module_geometry(element: &view_element::Module) -> ModuleGeometry {
    let cx = element.x;
    let cy = element.y;
    let w = MODULE_WIDTH;
    let h = MODULE_HEIGHT;

    ModuleGeometry {
        // TS uses Math.ceil for x and y
        rect: Frame {
            x: (cx - w / 2.0).ceil(),
            y: (cy - h / 2.0).ceil(),
            width: w,
            height: h,
        },
        corner_radius: MODULE_RADIUS,
        label: LabelProps::new(cx, cy, element.label_side, display_name(&element.name))
            .with_radii(w / 2.0, h / 2.0),
    }
}

pub fn render_module(element: &view_element::Module) -> String {
    let g = module_geometry(element);

    let mut svg = String::new();
    svg.push_str("<g class=\"simlin-module\">");
    svg.push_str(&format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" ry=\"{}\"></rect>",
        js_format_number(g.rect.x),
        js_format_number(g.rect.y),
        js_format_number(g.rect.width),
        js_format_number(g.rect.height),
        js_format_number(g.corner_radius),
        js_format_number(g.corner_radius)
    ));
    svg.push_str(&render_label(&g.label));
    svg.push_str("</g>");
    svg
}

/// The module's bare *shape* box (the rounded rect), WITHOUT its label. See
/// `aux_shape_bounds` for why the label-free shape is exposed separately.
pub(crate) fn module_shape_bounds(element: &view_element::Module) -> Rect {
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

/// The module's drawn extent: its shape and its label, as the TS Canvas's
/// `moduleBounds` measures it.
pub fn module_bounds(element: &view_element::Module) -> Rect {
    element_with_label_bounds(
        module_shape_bounds(element),
        &module_geometry(element).label,
    )
}

// --- Cloud ---

/// Where [`CLOUD_PATH`] is drawn for a cloud: the SVG
/// `matrix(scale, 0, 0, scale, tx, ty)` that maps the outline's design box
/// onto a circle of `CLOUD_RADIUS` around the cloud's center.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct CloudTransform {
    pub scale: f64,
    pub tx: f64,
    pub ty: f64,
}

impl CloudTransform {
    pub(crate) fn apply(&self, p: Point) -> Point {
        Point {
            x: self.scale * p.x + self.tx,
            y: self.scale * p.y + self.ty,
        }
    }
}

pub(crate) fn cloud_transform(element: &view_element::Cloud) -> CloudTransform {
    let radius = CLOUD_RADIUS;
    let diameter = radius * 2.0;
    CloudTransform {
        scale: diameter / CLOUD_WIDTH,
        tx: element.x - radius,
        ty: element.y - radius,
    }
}

pub fn render_cloud(element: &view_element::Cloud) -> String {
    let t = cloud_transform(element);

    let transform = format!(
        "matrix({}, 0, 0, {}, {}, {})",
        js_format_number(t.scale),
        js_format_number(t.scale),
        js_format_number(t.tx),
        js_format_number(t.ty)
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

/// Everything a drawn alias is made of.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub(crate) struct AliasGeometry {
    pub circle: Circle,
    pub label: LabelProps,
    /// The box the web canvas draws the aliased variable's sparkline in
    /// (`Alias.tsx`).
    pub sparkline: Frame,
}

/// `alias_of_name` is the name of the element the alias points at, `None`
/// when that element is missing from the view.
pub(crate) fn alias_geometry(
    element: &view_element::Alias,
    alias_of_name: Option<&str>,
) -> AliasGeometry {
    let cx = element.x;
    let cy = element.y;
    let r = AUX_RADIUS;
    let name = alias_of_name.unwrap_or(UNKNOWN_ALIAS_LABEL);

    // Alias hardcodes isArrayed = false
    AliasGeometry {
        circle: Circle { x: cx, y: cy, r },
        label: LabelProps::new(cx, cy, element.label_side, display_name(name)).with_radii(r, r),
        sparkline: Frame {
            x: cx + 1.0 - r / 2.0,
            y: cy + 1.0 - r / 2.0,
            width: r - 2.0,
            height: r - 2.0,
        },
    }
}

pub fn render_alias(element: &view_element::Alias, alias_of_name: Option<&str>) -> String {
    let g = alias_geometry(element, alias_of_name);

    let mut svg = String::new();
    svg.push_str("<g class=\"simlin-alias\">");
    svg.push_str(&svg_circle(&g.circle));
    svg.push_str(&render_label(&g.label));
    svg.push_str("</g>");
    svg
}

// --- Group ---

/// Everything a drawn group (sector box) is made of.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub(crate) struct GroupGeometry {
    pub rect: Frame,
    pub corner_radius: f64,
    /// The label's top-left anchor: SVG `dominant-baseline="hanging"` at
    /// this point.
    pub label_anchor: Point,
    pub label_text: String,
}

pub(crate) fn group_geometry(element: &view_element::Group) -> GroupGeometry {
    let left = element.x - element.width / 2.0;
    let top = element.y - element.height / 2.0;

    GroupGeometry {
        rect: Frame {
            x: left,
            y: top,
            width: element.width,
            height: element.height,
        },
        corner_radius: GROUP_RADIUS,
        label_anchor: Point {
            x: left + GROUP_LABEL_PADDING,
            y: top + GROUP_LABEL_PADDING,
        },
        label_text: display_name(&element.name),
    }
}

pub fn render_group(element: &view_element::Group) -> String {
    let g = group_geometry(element);

    let mut svg = String::new();
    svg.push_str("<g class=\"simlin-group\">");
    svg.push_str(&format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" ry=\"{}\"></rect>",
        js_format_number(g.rect.x),
        js_format_number(g.rect.y),
        js_format_number(g.rect.width),
        js_format_number(g.rect.height),
        js_format_number(g.corner_radius),
        js_format_number(g.corner_radius)
    ));
    svg.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" dominant-baseline=\"hanging\">",
        js_format_number(g.label_anchor.x),
        js_format_number(g.label_anchor.y)
    ));
    svg.push_str(&escape_xml_text(&g.label_text));
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
            compat: None,
        }
    }

    fn make_stock(x: f64, y: f64, name: &str) -> view_element::Stock {
        view_element::Stock {
            name: name.to_string(),
            uid: 2,
            x,
            y,
            label_side: LabelSide::Bottom,
            compat: None,
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
            compat: None,
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
            compat: None,
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
            compat: None,
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
            compat: None,
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
            is_mdl_view_marker: false,
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
            is_mdl_view_marker: false,
        };
        let bounds = group_bounds(&element);
        assert_eq!(bounds.left, 50.0);
        assert_eq!(bounds.top, 100.0);
        assert_eq!(bounds.right, 350.0);
        assert_eq!(bounds.bottom, 300.0);
    }
}
