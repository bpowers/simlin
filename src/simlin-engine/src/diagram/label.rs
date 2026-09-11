// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::datamodel::view_element::LabelSide;
use crate::diagram::common::{
    Rect, escape_xml_attr, escape_xml_text, js_format_number, merge_bounds,
};
use crate::diagram::constants::{
    AUX_RADIUS, LABEL_FONT_SIZE, LABEL_FONT_WEIGHT, LABEL_PADDING, LINE_SPACING,
};

#[cfg_attr(feature = "debug-derive", derive(Debug))]
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
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

/// How far one label line's anchor sits below the previous line's: the SVG
/// `dy` of its `<tspan>`, where `1em` is the label font size.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum LineAdvance {
    OneEm,
    Px(i64),
}

impl LineAdvance {
    fn svg(self) -> String {
        match self {
            LineAdvance::OneEm => "1em".to_string(),
            LineAdvance::Px(n) => format!("{n}px"),
        }
    }

    fn canvas_units(self) -> f64 {
        match self {
            LineAdvance::OneEm => LABEL_FONT_SIZE,
            LineAdvance::Px(n) => n as f64,
        }
    }
}

struct LabelLayout {
    text_x: f64,
    text_y: f64,
    x: f64,
    lines: Vec<String>,
    advances: Vec<LineAdvance>,
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
            text_y = cy - (LABEL_FONT_SIZE + (lines.len() as f64 - 1.0) * LINE_SPACING) / 2.0 - 3.0;
        }
        LabelSide::Right => {
            x = cx + rw + LABEL_PADDING;
            align = TextAnchor::Start;
            text_y = cy - (LABEL_FONT_SIZE + (lines.len() as f64 - 1.0) * LINE_SPACING) / 2.0 - 3.0;
        }
        LabelSide::Center => {
            // TS falls through to default in the switch, which logs a warning
            // and uses the initial values (textY = cy, align = middle)
        }
    }

    let count = lines.len() as i64;
    let advances = (0..lines.len())
        .map(|i| {
            if reverse_baseline && i == 0 {
                // A label above its element stacks upward: its first line
                // starts (count - 1) lines above the anchor so the last line
                // sits on it.
                LineAdvance::Px(-(LINE_SPACING as i64 * (count - 1)))
            } else if i == 0 {
                LineAdvance::OneEm
            } else {
                LineAdvance::Px(LINE_SPACING as i64)
            }
        })
        .collect();

    LabelLayout {
        text_x,
        text_y,
        x,
        lines,
        advances,
        align,
    }
}

/// One label line placed in canvas units.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub(crate) struct LabelLine {
    pub text: String,
    pub x: f64,
    pub y: f64,
}

/// The label's text anchor and every line's anchor point: the SVG `<text>`'s
/// `y` advanced by each `<tspan>`'s `dy` in turn, at each tspan's absolute
/// `x`. `render_label` prints this same layout, so a display list that places
/// lines at these points places them where the SVG does.
pub(crate) fn label_lines(props: &LabelProps) -> (TextAnchor, Vec<LabelLine>) {
    let layout = label_layout(props);
    let x = layout.x;
    let mut y = layout.text_y;
    let lines = layout
        .lines
        .into_iter()
        .zip(layout.advances)
        .map(|(text, advance)| {
            y += advance.canvas_units();
            LabelLine { text, x, y }
        })
        .collect();
    (layout.align, lines)
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
            text_y = cy - (LABEL_FONT_SIZE + (lines.len() as f64 - 1.0) * LINE_SPACING) / 2.0 - 3.0;
            x - editor_width
        }
        LabelSide::Right => {
            let x = cx + rw + LABEL_PADDING - 1.0;
            text_y = cy - (LABEL_FONT_SIZE + (lines.len() as f64 - 1.0) * LINE_SPACING) / 2.0 - 3.0;
            x
        }
        LabelSide::Center => text_x - editor_width / 2.0,
    };

    text_y = text_y.round();

    Rect {
        top: text_y,
        left,
        right: left + editor_width,
        bottom: text_y + LINE_SPACING * lines_count as f64,
    }
}

pub fn render_label(props: &LabelProps) -> String {
    let layout = label_layout(props);

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

    // Font properties are inlined rather than relying on CSS <style> blocks alone,
    // because resvg-wasm >= 0.4 doesn't apply CSS class-based font properties to text.
    // Single quotes (&#x27;) avoid &quot; encoding issues with React's renderToString.
    // We use &#x27; here to match React's encoding of single quotes in attributes.
    svg.push_str(&format!(
        " style=\"fill:#000000;font-size:{}px;font-family:&#x27;Roboto Light&#x27;, &#x27;Roboto&#x27;, &#x27;Open Sans&#x27;, &#x27;Arial&#x27;, sans-serif;font-weight:{};white-space:nowrap;text-anchor:{};filter:url(#labelBackground)\"",
        js_format_number(LABEL_FONT_SIZE),
        LABEL_FONT_WEIGHT,
        layout.align.as_str()
    ));

    svg.push_str(" text-rendering=\"optimizeLegibility\">");

    for (line, advance) in layout.lines.iter().zip(&layout.advances) {
        svg.push_str(&format!(
            "<tspan x=\"{}\" dy=\"{}\">",
            escape_xml_attr(&js_format_number(layout.x)),
            escape_xml_attr(&advance.svg())
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

    /// The expected anchor, line x, single-line y, and two-line ys for an
    /// aux-radius element at (100, 200) on `side`. The match has no wildcard
    /// arm, so a new `LabelSide` does not compile until it has a row here; the
    /// row list in the test below enumerates the same variants.
    fn expected_lines(side: LabelSide) -> (TextAnchor, f64, f64, [f64; 2]) {
        match side {
            // text_y = 200 - 9 - 4 - 2 = 185; the last line sits on it.
            LabelSide::Top => (TextAnchor::Middle, 100.0, 185.0, [171.0, 185.0]),
            // text_y = 200 + 9 + 4 = 213; 1em = 12, then 14 per line.
            LabelSide::Bottom => (TextAnchor::Middle, 100.0, 225.0, [225.0, 239.0]),
            // x = 100 - 9 - 4; text_y = 200 - (12 + 14 * (n - 1)) / 2 - 3.
            LabelSide::Left => (TextAnchor::End, 87.0, 203.0, [196.0, 210.0]),
            LabelSide::Right => (TextAnchor::Start, 113.0, 203.0, [196.0, 210.0]),
            // text_y = cy.
            LabelSide::Center => (TextAnchor::Middle, 100.0, 212.0, [212.0, 226.0]),
        }
    }

    /// The `(x, y)` of every tspan in `svg`, applying SVG's own `dy`
    /// semantics (`1em` is the 12px font size) to the `<text>`'s `y`. This is
    /// the oracle `label_lines` must agree with: what the printed SVG places.
    fn svg_line_anchors(svg: &str) -> Vec<(f64, f64)> {
        let attr = |tag: &str, name: &str| -> f64 {
            let key = format!(" {name}=\"");
            let start = tag.find(&key).unwrap() + key.len();
            let end = start + tag[start..].find('"').unwrap();
            tag[start..end].parse().unwrap()
        };
        let text_tag = &svg[svg.find("<text").unwrap()..];
        let mut y = attr(text_tag, "y");
        let mut anchors = Vec::new();
        for tspan in svg.split("<tspan").skip(1) {
            let dy_start = tspan.find(" dy=\"").unwrap() + 5;
            let dy_end = dy_start + tspan[dy_start..].find('"').unwrap();
            let dy = &tspan[dy_start..dy_end];
            y += if dy == "1em" {
                12.0
            } else {
                dy.trim_end_matches("px").parse::<f64>().unwrap()
            };
            anchors.push((attr(tspan, "x"), y));
        }
        anchors
    }

    #[test]
    fn label_lines_place_every_line_for_every_side() {
        let sides = [
            LabelSide::Top,
            LabelSide::Left,
            LabelSide::Center,
            LabelSide::Bottom,
            LabelSide::Right,
        ];
        for side in sides {
            let (anchor, x, one_y, two_ys) = expected_lines(side);

            let one = LabelProps::new(100.0, 200.0, side, "alpha".to_string());
            let (one_anchor, one_lines) = label_lines(&one);
            assert!(one_anchor == anchor, "{side:?}: anchor");
            assert_eq!(
                one_lines,
                vec![LabelLine {
                    text: "alpha".to_string(),
                    x,
                    y: one_y
                }],
                "{side:?}: one line"
            );
            assert_eq!(
                svg_line_anchors(&render_label(&one)),
                vec![(x, one_y)],
                "{side:?}: the SVG places the line at the same point"
            );

            let two = LabelProps::new(100.0, 200.0, side, "alpha\nbeta".to_string());
            let (two_anchor, two_lines) = label_lines(&two);
            assert!(two_anchor == anchor, "{side:?}: two-line anchor");
            assert_eq!(
                two_lines,
                vec![
                    LabelLine {
                        text: "alpha".to_string(),
                        x,
                        y: two_ys[0]
                    },
                    LabelLine {
                        text: "beta".to_string(),
                        x,
                        y: two_ys[1]
                    },
                ],
                "{side:?}: two lines"
            );
            assert_eq!(
                svg_line_anchors(&render_label(&two)),
                vec![(x, two_ys[0]), (x, two_ys[1])],
                "{side:?}: the SVG places both lines at the same points"
            );
        }
    }
}
