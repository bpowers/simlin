// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::datamodel::view_element::LabelSide;
use crate::diagram::common::{
    Rect, escape_xml_attr, escape_xml_text, js_format_number, merge_bounds,
};
use crate::diagram::constants::{AUX_RADIUS, LABEL_PADDING, LINE_SPACING};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TextAnchor {
    Start,
    Middle,
    End,
}

impl TextAnchor {
    fn as_str(&self) -> &'static str {
        match self {
            TextAnchor::Start => "start",
            TextAnchor::Middle => "middle",
            TextAnchor::End => "end",
        }
    }
}

pub struct LabelProps {
    pub cx: f64,
    pub cy: f64,
    pub side: LabelSide,
    pub rw: f64,
    pub rh: f64,
    pub text: String,
}

impl LabelProps {
    pub fn new(cx: f64, cy: f64, side: LabelSide, text: String) -> Self {
        LabelProps {
            cx,
            cy,
            side,
            rw: AUX_RADIUS,
            rh: AUX_RADIUS,
            text,
        }
    }

    pub fn with_radii(mut self, rw: f64, rh: f64) -> Self {
        self.rw = rw;
        self.rh = rh;
        self
    }
}

struct LabelLayout {
    text_x: f64,
    text_y: f64,
    x: f64,
    lines: Vec<String>,
    reverse_baseline: bool,
    align: TextAnchor,
}

fn label_layout(props: &LabelProps) -> LabelLayout {
    let lines: Vec<String> = props.text.split('\n').map(|s| s.to_string()).collect();

    let cx = props.cx;
    let cy = props.cy;
    let rw = props.rw;
    let rh = props.rh;
    let mut x = cx;
    let text_x = x;
    let mut text_y = cy;
    let mut align = TextAnchor::Middle;
    let mut reverse_baseline = false;

    match props.side {
        LabelSide::Top => {
            reverse_baseline = true;
            text_y = cy - rh - LABEL_PADDING - 2.0;
        }
        LabelSide::Bottom => {
            text_y = cy + rh + LABEL_PADDING;
        }
        LabelSide::Left => {
            x = cx - rw - LABEL_PADDING;
            align = TextAnchor::End;
            text_y = cy - (12.0 + (lines.len() as f64 - 1.0) * 14.0) / 2.0 - 3.0;
        }
        LabelSide::Right => {
            x = cx + rw + LABEL_PADDING;
            align = TextAnchor::Start;
            text_y = cy - (12.0 + (lines.len() as f64 - 1.0) * 14.0) / 2.0 - 3.0;
        }
        LabelSide::Center => {
            // TS falls through to default in the switch, which logs a warning
            // and uses the initial values (textY = cy, align = middle)
        }
    }

    LabelLayout {
        text_x,
        text_y,
        x,
        lines,
        reverse_baseline,
        align,
    }
}

pub fn label_bounds(props: &LabelProps) -> Rect {
    let lines: Vec<&str> = props.text.split('\n').collect();
    let lines_count = lines.len();

    let max_width_chars = lines.iter().map(|l| l.len()).max().unwrap_or(0);
    let editor_width = max_width_chars as f64 * 6.0 + 10.0;

    let cx = props.cx;
    let cy = props.cy;
    let rw = props.rw;
    let rh = props.rh;
    let text_x = cx;
    let mut text_y = cy;

    let left = match props.side {
        LabelSide::Top => {
            text_y = cy - rh - LABEL_PADDING - LINE_SPACING * lines_count as f64;
            text_x - editor_width / 2.0
        }
        LabelSide::Bottom => {
            text_y = cy + rh + LABEL_PADDING;
            text_x - editor_width / 2.0
        }
        LabelSide::Left => {
            let x = cx - rw - LABEL_PADDING + 1.0;
            text_y = cy - (12.0 + (lines.len() as f64 - 1.0) * 14.0) / 2.0 - 3.0;
            x - editor_width
        }
        LabelSide::Right => {
            let x = cx + rw + LABEL_PADDING - 1.0;
            text_y = cy - (12.0 + (lines.len() as f64 - 1.0) * 14.0) / 2.0 - 3.0;
            x
        }
        LabelSide::Center => text_x - editor_width / 2.0,
    };

    text_y = text_y.round();

    Rect {
        top: text_y,
        left,
        right: left + editor_width,
        bottom: text_y + 14.0 * lines_count as f64,
    }
}

pub fn render_label(props: &LabelProps) -> String {
    let layout = label_layout(props);
    let lines_count = layout.lines.len();

    let mut svg = String::new();

    // React SSR converts textAnchor to text-anchor, textRendering to text-rendering
    svg.push_str("<g><text");
    svg.push_str(&format!(
        " x=\"{}\"",
        escape_xml_attr(&js_format_number(layout.text_x))
    ));
    svg.push_str(&format!(
        " y=\"{}\"",
        escape_xml_attr(&js_format_number(layout.text_y))
    ));

    // The TS code always includes text-anchor when align is truthy ('middle' is truthy)
    svg.push_str(&format!(
        " style=\"text-anchor:{};filter:url(#labelBackground)\"",
        layout.align.as_str()
    ));

    svg.push_str(" text-rendering=\"optimizeLegibility\">");

    for (i, line) in layout.lines.iter().enumerate() {
        let dy = if layout.reverse_baseline && i == 0 {
            format!("{}px", -(LINE_SPACING as i64 * (lines_count as i64 - 1)))
        } else if i == 0 {
            "1em".to_string()
        } else {
            format!("{}px", LINE_SPACING as i64)
        };

        svg.push_str(&format!(
            "<tspan x=\"{}\" dy=\"{}\">",
            escape_xml_attr(&js_format_number(layout.x)),
            escape_xml_attr(&dy)
        ));
        svg.push_str(&escape_xml_text(line));
        svg.push_str("</tspan>");
    }

    svg.push_str("</text></g>");
    svg
}

/// Combined bounds: merge element bounds with label bounds
pub fn element_with_label_bounds(element_bounds: Rect, label_props: &LabelProps) -> Rect {
    merge_bounds(element_bounds, label_bounds(label_props))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_label_bounds_bottom() {
        let props = LabelProps::new(100.0, 100.0, LabelSide::Bottom, "test".to_string());
        let bounds = label_bounds(&props);
        assert!(bounds.top > 100.0); // label is below element
        assert!(bounds.bottom > bounds.top);
    }

    #[test]
    fn test_label_bounds_top() {
        let props = LabelProps::new(100.0, 100.0, LabelSide::Top, "test".to_string());
        let bounds = label_bounds(&props);
        assert!(bounds.bottom < 100.0); // label is above element
    }

    #[test]
    fn test_label_bounds_left() {
        let props = LabelProps::new(100.0, 100.0, LabelSide::Left, "test".to_string());
        let bounds = label_bounds(&props);
        assert!(bounds.right <= 100.0); // label is to the left
    }

    #[test]
    fn test_label_bounds_right() {
        let props = LabelProps::new(100.0, 100.0, LabelSide::Right, "test".to_string());
        let bounds = label_bounds(&props);
        assert!(bounds.left >= 100.0); // label is to the right
    }

    #[test]
    fn test_label_bounds_center() {
        let props = LabelProps::new(100.0, 100.0, LabelSide::Center, "test".to_string());
        let bounds = label_bounds(&props);
        // Center label is around the element center
        assert!(bounds.left < 100.0);
        assert!(bounds.right > 100.0);
    }

    #[test]
    fn test_label_bounds_multiline() {
        let props = LabelProps::new(100.0, 100.0, LabelSide::Bottom, "line1\nline2".to_string());
        let bounds = label_bounds(&props);
        let single_props = LabelProps::new(100.0, 100.0, LabelSide::Bottom, "line1".to_string());
        let single_bounds = label_bounds(&single_props);
        // Multiline should be taller
        assert!(bounds.bottom - bounds.top > single_bounds.bottom - single_bounds.top);
    }

    #[test]
    fn test_render_label_basic() {
        let props = LabelProps::new(100.0, 200.0, LabelSide::Bottom, "test".to_string());
        let svg = render_label(&props);
        assert!(svg.starts_with("<g><text"));
        assert!(svg.ends_with("</text></g>"));
        assert!(svg.contains("text-rendering=\"optimizeLegibility\""));
        assert!(svg.contains("text-anchor:"));
        assert!(svg.contains("<tspan"));
        assert!(svg.contains(">test</tspan>"));
    }

    #[test]
    fn test_render_label_multiline() {
        let props = LabelProps::new(100.0, 200.0, LabelSide::Bottom, "line1\nline2".to_string());
        let svg = render_label(&props);
        assert!(svg.contains(">line1</tspan>"));
        assert!(svg.contains(">line2</tspan>"));
        assert!(svg.contains("dy=\"1em\""));
        assert!(svg.contains("dy=\"14px\""));
    }

    #[test]
    fn test_render_label_top_reverse_baseline() {
        let props = LabelProps::new(100.0, 200.0, LabelSide::Top, "line1\nline2".to_string());
        let svg = render_label(&props);
        // First tspan should have negative dy for reverse baseline
        assert!(svg.contains("dy=\"-14px\""));
    }

    #[test]
    fn test_render_label_escaping() {
        let props = LabelProps::new(100.0, 200.0, LabelSide::Bottom, "a & b".to_string());
        let svg = render_label(&props);
        assert!(svg.contains(">a &amp; b</tspan>"));
    }

    #[test]
    fn test_label_layout_left_align() {
        let props = LabelProps::new(100.0, 200.0, LabelSide::Left, "test".to_string());
        let svg = render_label(&props);
        assert!(svg.contains("text-anchor:end"));
    }

    #[test]
    fn test_label_layout_right_align() {
        let props = LabelProps::new(100.0, 200.0, LabelSide::Right, "test".to_string());
        let svg = render_label(&props);
        assert!(svg.contains("text-anchor:start"));
    }
}
