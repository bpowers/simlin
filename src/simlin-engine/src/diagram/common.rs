// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::f64::consts::PI;

#[derive(Clone, Copy, PartialEq)]
pub struct Rect {
    pub top: f64,
    pub left: f64,
    pub right: f64,
    pub bottom: f64,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Circle {
    pub x: f64,
    pub y: f64,
    pub r: f64,
}

/// Replaces `\\n` with newline and `_` with space, matching the TS displayName
pub fn display_name(name: &str) -> String {
    name.replace("\\n", "\n").replace('_', " ")
}

/// Escape text content for XML (inside elements)
pub fn escape_xml_text(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            _ => result.push(c),
        }
    }
    result
}

/// Escape attribute values for XML (inside double-quoted attributes)
pub fn escape_xml_attr(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '"' => result.push_str("&quot;"),
            _ => result.push(c),
        }
    }
    result
}

/// Format a floating point number for emission into an SVG attribute.
///
/// Mirrors JavaScript's `Number.toString()` (no trailing `.0` for integers, no
/// trailing zeros) and additionally **quantizes the value to 6 decimal places**
/// before formatting. Quantization is what keeps Rust SVG and TypeScript SVG
/// byte-identical (`src/diagram/tests/svg-rendering.test.ts` and the connector
/// arc regression guard `diagram::connector::tests::test_render_arc_svg_byte_identical`)
/// across compiler/hardware variation: 1-ULP f64 differences (e.g.
/// 273.2050807568877 vs 273.20508075688764) collapse to the same printed
/// string. Sub-micropixel precision is far above any visible rendering
/// threshold and well below the 7e-14 ULP at coordinate magnitudes around 300.
///
/// The TypeScript helper `jsFormatNumber` in `src/diagram/render-common.tsx`
/// must mirror this function exactly (same precision, same trimming, same
/// NaN/Infinity strings, same -0 normalization).
pub fn js_format_number(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
    }

    // Round to 6 decimal places, then renormalize -0 (so a tiny negative input
    // that rounded down to zero doesn't print as "-0").
    let mut r = (n * 1e6).round() / 1e6;
    if r == 0.0 {
        r = 0.0;
    }

    // For integers (after quantization), JS outputs no decimal point.
    if r == r.trunc() && r.abs() < 1e21 {
        return format!("{}", r as i64);
    }

    // Format with up to 6 fractional digits, then strip trailing zeros (and a
    // dangling decimal point) so "0.5" stays "0.5" rather than "0.500000".
    let s = format!("{r:.6}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    trimmed.to_string()
}

pub fn merge_bounds(a: Rect, b: Rect) -> Rect {
    Rect {
        top: a.top.min(b.top),
        left: a.left.min(b.left),
        right: a.right.max(b.right),
        bottom: a.bottom.max(b.bottom),
    }
}

pub fn calc_view_box(bounds: &[Option<Rect>]) -> Option<Rect> {
    if bounds.is_empty() {
        return None;
    }

    let initial = Rect {
        top: f64::INFINITY,
        left: f64::INFINITY,
        right: f64::NEG_INFINITY,
        bottom: f64::NEG_INFINITY,
    };

    let result = bounds
        .iter()
        .fold(initial, |view, maybe_box| match maybe_box {
            Some(b) => merge_bounds(view, *b),
            None => view,
        });

    // If we never merged anything meaningful, return None
    if result.top == f64::INFINITY {
        return None;
    }

    Some(result)
}

pub fn is_zero(n: f64) -> bool {
    n.abs() < 0.0000001
}

pub fn is_inf(n: f64) -> bool {
    !n.is_finite() || n > 2e14
}

pub fn square(n: f64) -> f64 {
    n * n
}

pub fn deg_to_rad(d: f64) -> f64 {
    (d / 180.0) * PI
}

pub fn rad_to_deg(r: f64) -> f64 {
    (r * 180.0) / PI
}

// These rectangle/segment geometry primitives are the load-bearing helpers for
// the layout quality metric (`layout::metrics`). `rect_width`/`rect_height`/
// `rect_area`/`rect_overlap_area` are consumed there (node-overlap,
// label-overlap, sprawl, and aspect terms), and `segment_clip_interval_in_rect`
// is the Liang-Barsky core that `node_connector_overlap` unions across boxes.
// `rect_contains_point` and `segment_length_in_rect` are primitives kept for
// completeness and as the single-box reference oracle the metric's tests check
// the union path against, so each stays `#[allow(dead_code)]` until a non-test
// caller needs it.

/// Width of a rect (right - left). May be negative for a degenerate/inverted rect.
pub(crate) fn rect_width(r: &Rect) -> f64 {
    r.right - r.left
}

/// Height of a rect (bottom - top).
pub(crate) fn rect_height(r: &Rect) -> f64 {
    r.bottom - r.top
}

/// Area of a rect, clamped to >= 0.
pub(crate) fn rect_area(r: &Rect) -> f64 {
    (rect_width(r).max(0.0)) * (rect_height(r).max(0.0))
}

/// Area of the axis-aligned intersection of two rects (0 if they do not overlap).
pub(crate) fn rect_overlap_area(a: &Rect, b: &Rect) -> f64 {
    let w = a.right.min(b.right) - a.left.max(b.left);
    let h = a.bottom.min(b.bottom) - a.top.max(b.top);
    if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
}

/// True if `p` lies inside (or on the boundary of) `r`.
#[allow(dead_code)]
pub(crate) fn rect_contains_point(r: &Rect, p: &Point) -> bool {
    p.x >= r.left && p.x <= r.right && p.y >= r.top && p.y <= r.bottom
}

/// Clipped parameter interval `[t0, t1]` of segment `p0 + t*(p1-p0)` (t in
/// [0,1]) that lies within axis-aligned rect `r`, or `None` if the segment never
/// enters `r`. When `Some`, `0.0 <= t0 < t1 <= 1.0` (a zero-thickness touch
/// where `t0 == t1` returns `None`, contributing no length). This is the
/// Liang-Barsky core; `segment_length_in_rect` delegates to it, and
/// `layout::metrics` uses the raw intervals to UNION a connector's coverage
/// across multiple boxes so each physical sub-length is counted at most once.
/// Pure; no allocation.
pub(crate) fn segment_clip_interval_in_rect(
    p0: &Point,
    p1: &Point,
    r: &Rect,
) -> Option<(f64, f64)> {
    // Liang-Barsky clip of the parametric segment p0 + t*(p1-p0), t in [0,1],
    // against left/right/top/bottom slabs.
    let dx = p1.x - p0.x;
    let dy = p1.y - p0.y;
    let mut t0 = 0.0_f64;
    let mut t1 = 1.0_f64;
    // (p, q) pairs for the four half-planes; segment inside slab where p*t <= q.
    let edges = [
        (-dx, p0.x - r.left),
        (dx, r.right - p0.x),
        (-dy, p0.y - r.top),
        (dy, r.bottom - p0.y),
    ];
    for (p, q) in edges {
        if p == 0.0 {
            if q < 0.0 {
                return None; // parallel and outside this slab
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                if t > t1 {
                    return None;
                }
                if t > t0 {
                    t0 = t;
                }
            } else {
                if t < t0 {
                    return None;
                }
                if t < t1 {
                    t1 = t;
                }
            }
        }
    }
    if t1 > t0 { Some((t0, t1)) } else { None }
}

/// Length of the portion of segment p0->p1 that lies within axis-aligned rect r.
/// Returns 0 if the segment never enters r. Pure; no allocation. Delegates to
/// `segment_clip_interval_in_rect` so the clip math lives in exactly one place.
#[allow(dead_code)]
pub(crate) fn segment_length_in_rect(p0: &Point, p1: &Point, r: &Rect) -> f64 {
    match segment_clip_interval_in_rect(p0, p1, r) {
        Some((t0, t1)) => {
            let dx = p1.x - p0.x;
            let dy = p1.y - p0.y;
            let seg_len = (dx * dx + dy * dy).sqrt();
            (t1 - t0) * seg_len
        }
        None => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_name_basic() {
        assert_eq!(display_name("hello_world"), "hello world");
        assert_eq!(display_name("line1\\nline2"), "line1\nline2");
        assert_eq!(display_name("no_changes\\nhere"), "no changes\nhere");
        assert_eq!(display_name(""), "");
        assert_eq!(display_name("plain"), "plain");
    }

    #[test]
    fn test_escape_xml_text() {
        assert_eq!(escape_xml_text("hello"), "hello");
        assert_eq!(escape_xml_text("a & b"), "a &amp; b");
        assert_eq!(escape_xml_text("<tag>"), "&lt;tag&gt;");
        assert_eq!(escape_xml_text(""), "");
        assert_eq!(escape_xml_text("a < b & c > d"), "a &lt; b &amp; c &gt; d");
    }

    #[test]
    fn test_escape_xml_attr() {
        assert_eq!(escape_xml_attr("hello"), "hello");
        assert_eq!(escape_xml_attr("a & b"), "a &amp; b");
        assert_eq!(escape_xml_attr("<tag>"), "&lt;tag&gt;");
        assert_eq!(escape_xml_attr("say \"hi\""), "say &quot;hi&quot;");
        assert_eq!(escape_xml_attr(""), "");
    }

    #[test]
    fn test_js_format_number() {
        assert_eq!(js_format_number(45.0), "45");
        assert_eq!(js_format_number(0.0), "0");
        assert_eq!(js_format_number(-0.0), "0");
        assert_eq!(js_format_number(0.5), "0.5");
        assert_eq!(js_format_number(-3.125), "-3.125");
        assert_eq!(js_format_number(1.0), "1");
        assert_eq!(js_format_number(-1.0), "-1");
        assert_eq!(js_format_number(100.0), "100");
        assert_eq!(js_format_number(f64::NAN), "NaN");
        assert_eq!(js_format_number(f64::INFINITY), "Infinity");
        assert_eq!(js_format_number(f64::NEG_INFINITY), "-Infinity");
    }

    /// Coordinates emitted into SVG are quantized to 6 decimal places (well
    /// below any visible threshold) so 1-ULP f64 differences from compiler or
    /// hardware variation no longer leak into the byte-identical SVG parity
    /// (`svg-rendering.test.ts` and the connector arc regression guard).
    /// The TypeScript `jsFormatNumber` helper in `src/diagram/render-common.tsx`
    /// must mirror these cases exactly.
    #[test]
    fn test_js_format_number_quantizes_to_six_decimals() {
        // A value that rounds to a "clean" integer collapses to that integer.
        assert_eq!(js_format_number(100.0000004), "100");
        // A value above .5 in the seventh decimal rounds up.
        assert_eq!(js_format_number(0.1234567), "0.123457");
        // A value below .5 in the seventh decimal rounds down (banker's-style
        // rounding does NOT apply here -- `(x * 1e6).round()` is half-away-
        // from-zero, matching JS `Math.round` to-half-up for positives).
        assert_eq!(js_format_number(0.1234564), "0.123456");
        // 1-ULP siblings of a 6-decimal-clean number both collapse to it.
        let clean = 273.205081_f64;
        let ulp_above = f64::from_bits(clean.to_bits() + 1);
        let ulp_below = f64::from_bits(clean.to_bits() - 1);
        assert_eq!(js_format_number(ulp_above), js_format_number(clean));
        assert_eq!(js_format_number(ulp_below), js_format_number(clean));
        // The two ULP-different arc-radius values from the bug repro both
        // collapse to the same printed string.
        assert_eq!(
            js_format_number(273.2050807568877),
            js_format_number(273.20508075688764)
        );
        // Trailing zeros are trimmed.
        assert_eq!(js_format_number(1.5), "1.5");
        assert_eq!(js_format_number(1.500000), "1.5");
        // A value whose fractional part rounds to .0 prints without a decimal.
        assert_eq!(js_format_number(2.0000004), "2");
        // -0 normalizes to 0 even after rounding.
        assert_eq!(js_format_number(-0.0000001), "0");
        // Negative values round symmetrically to positive ones.
        assert_eq!(js_format_number(-0.1234567), "-0.123457");
    }

    #[test]
    fn test_merge_bounds() {
        let a = Rect {
            top: 10.0,
            left: 20.0,
            right: 30.0,
            bottom: 40.0,
        };
        let b = Rect {
            top: 5.0,
            left: 25.0,
            right: 35.0,
            bottom: 45.0,
        };
        let merged = merge_bounds(a, b);
        assert_eq!(merged.top, 5.0);
        assert_eq!(merged.left, 20.0);
        assert_eq!(merged.right, 35.0);
        assert_eq!(merged.bottom, 45.0);
    }

    #[test]
    fn test_calc_view_box_empty() {
        assert!(calc_view_box(&[]).is_none());
    }

    #[test]
    fn test_calc_view_box_all_none() {
        assert!(calc_view_box(&[None, None]).is_none());
    }

    #[test]
    fn test_calc_view_box_single() {
        let r = Rect {
            top: 10.0,
            left: 20.0,
            right: 30.0,
            bottom: 40.0,
        };
        let result = calc_view_box(&[Some(r)]).unwrap();
        assert_eq!(result.top, 10.0);
        assert_eq!(result.left, 20.0);
        assert_eq!(result.right, 30.0);
        assert_eq!(result.bottom, 40.0);
    }

    #[test]
    fn test_calc_view_box_mixed() {
        let r1 = Rect {
            top: 10.0,
            left: 20.0,
            right: 30.0,
            bottom: 40.0,
        };
        let r2 = Rect {
            top: 5.0,
            left: 25.0,
            right: 35.0,
            bottom: 45.0,
        };
        let result = calc_view_box(&[Some(r1), None, Some(r2)]).unwrap();
        assert_eq!(result.top, 5.0);
        assert_eq!(result.left, 20.0);
        assert_eq!(result.right, 35.0);
        assert_eq!(result.bottom, 45.0);
    }

    #[test]
    fn test_is_zero() {
        assert!(is_zero(0.0));
        assert!(is_zero(0.00000001));
        assert!(!is_zero(0.001));
        assert!(!is_zero(1.0));
    }

    #[test]
    fn test_is_inf() {
        assert!(is_inf(f64::INFINITY));
        assert!(is_inf(f64::NEG_INFINITY));
        assert!(is_inf(f64::NAN));
        assert!(is_inf(3e14));
        assert!(!is_inf(1e14));
        assert!(!is_inf(0.0));
    }

    #[test]
    fn test_square() {
        assert_eq!(square(3.0), 9.0);
        assert_eq!(square(0.0), 0.0);
        assert_eq!(square(-2.0), 4.0);
    }

    #[test]
    fn test_deg_rad_conversion() {
        assert!((deg_to_rad(180.0) - PI).abs() < 1e-10);
        assert!((deg_to_rad(90.0) - PI / 2.0).abs() < 1e-10);
        assert!((rad_to_deg(PI) - 180.0).abs() < 1e-10);
        assert!((rad_to_deg(PI / 2.0) - 90.0).abs() < 1e-10);
    }

    #[test]
    fn test_rect_dimensions() {
        let r = Rect {
            top: 10.0,
            left: 20.0,
            right: 50.0,
            bottom: 70.0,
        };
        assert_eq!(rect_width(&r), 30.0);
        assert_eq!(rect_height(&r), 60.0);
        assert_eq!(rect_area(&r), 30.0 * 60.0);
    }

    #[test]
    fn test_rect_area_clamps_negative() {
        // An inverted/degenerate rect (right < left, bottom < top) has
        // negative width/height; rect_area clamps each to 0 so the result is 0.
        let inverted = Rect {
            top: 70.0,
            left: 50.0,
            right: 20.0,
            bottom: 10.0,
        };
        assert!(rect_width(&inverted) < 0.0);
        assert!(rect_height(&inverted) < 0.0);
        assert_eq!(rect_area(&inverted), 0.0);
    }

    #[test]
    fn test_rect_overlap_area_known_overlap() {
        // a covers x in [0,10], y in [0,10]; b covers x in [5,15], y in [5,15].
        // Their intersection is x in [5,10], y in [5,10] => 5 x 5 = 25.
        let a = Rect {
            top: 0.0,
            left: 0.0,
            right: 10.0,
            bottom: 10.0,
        };
        let b = Rect {
            top: 5.0,
            left: 5.0,
            right: 15.0,
            bottom: 15.0,
        };
        assert_eq!(rect_overlap_area(&a, &b), 25.0);
        // Overlap is symmetric in argument order.
        assert_eq!(rect_overlap_area(&b, &a), 25.0);
    }

    #[test]
    fn test_rect_overlap_area_disjoint() {
        let a = Rect {
            top: 0.0,
            left: 0.0,
            right: 10.0,
            bottom: 10.0,
        };
        let b = Rect {
            top: 20.0,
            left: 20.0,
            right: 30.0,
            bottom: 30.0,
        };
        assert_eq!(rect_overlap_area(&a, &b), 0.0);
    }

    #[test]
    fn test_rect_overlap_area_identical() {
        // Two identical rects overlap by their full area.
        let r = Rect {
            top: 0.0,
            left: 0.0,
            right: 10.0,
            bottom: 4.0,
        };
        assert_eq!(rect_overlap_area(&r, &r), rect_area(&r));
        assert_eq!(rect_overlap_area(&r, &r), 40.0);
    }

    #[test]
    fn test_rect_overlap_area_touching_edge() {
        // b's left edge touches a's right edge (both at x=10): zero-width overlap => 0.
        let a = Rect {
            top: 0.0,
            left: 0.0,
            right: 10.0,
            bottom: 10.0,
        };
        let b = Rect {
            top: 0.0,
            left: 10.0,
            right: 20.0,
            bottom: 10.0,
        };
        assert_eq!(rect_overlap_area(&a, &b), 0.0);
    }

    #[test]
    fn test_rect_contains_point() {
        let r = Rect {
            top: 0.0,
            left: 0.0,
            right: 10.0,
            bottom: 10.0,
        };
        // Strictly inside.
        assert!(rect_contains_point(&r, &Point { x: 5.0, y: 5.0 }));
        // On the boundary (inclusive).
        assert!(rect_contains_point(&r, &Point { x: 0.0, y: 0.0 }));
        assert!(rect_contains_point(&r, &Point { x: 10.0, y: 10.0 }));
        assert!(rect_contains_point(&r, &Point { x: 0.0, y: 5.0 }));
        // Outside on each side.
        assert!(!rect_contains_point(&r, &Point { x: -1.0, y: 5.0 }));
        assert!(!rect_contains_point(&r, &Point { x: 11.0, y: 5.0 }));
        assert!(!rect_contains_point(&r, &Point { x: 5.0, y: -1.0 }));
        assert!(!rect_contains_point(&r, &Point { x: 5.0, y: 11.0 }));
    }

    #[test]
    fn test_segment_length_in_rect_crosses_fully() {
        // Rect spans x in [0,10], y in [0,10]. A horizontal segment from
        // (-5, 5) to (15, 5) enters at x=0 and exits at x=10 => inside length 10.
        let r = Rect {
            top: 0.0,
            left: 0.0,
            right: 10.0,
            bottom: 10.0,
        };
        let got =
            segment_length_in_rect(&Point { x: -5.0, y: 5.0 }, &Point { x: 15.0, y: 5.0 }, &r);
        assert!((got - 10.0).abs() < 1e-9, "got {got}");
    }

    #[test]
    fn test_segment_length_in_rect_entirely_outside() {
        let r = Rect {
            top: 0.0,
            left: 0.0,
            right: 10.0,
            bottom: 10.0,
        };
        // Segment well above the rect, never enters.
        let got =
            segment_length_in_rect(&Point { x: -5.0, y: 50.0 }, &Point { x: 15.0, y: 50.0 }, &r);
        assert_eq!(got, 0.0);
    }

    #[test]
    fn test_segment_length_in_rect_entirely_inside() {
        let r = Rect {
            top: 0.0,
            left: 0.0,
            right: 10.0,
            bottom: 10.0,
        };
        // Segment from (2,2) to (5,6): both endpoints inside; full length is
        // sqrt(3^2 + 4^2) = 5.
        let got = segment_length_in_rect(&Point { x: 2.0, y: 2.0 }, &Point { x: 5.0, y: 6.0 }, &r);
        assert!((got - 5.0).abs() < 1e-9, "got {got}");
    }

    #[test]
    fn test_segment_length_in_rect_one_endpoint_inside() {
        let r = Rect {
            top: 0.0,
            left: 0.0,
            right: 10.0,
            bottom: 10.0,
        };
        // Horizontal segment from (5,5) (inside) to (25,5) (outside): the
        // portion inside runs from x=5 to x=10 => length 5.
        let got = segment_length_in_rect(&Point { x: 5.0, y: 5.0 }, &Point { x: 25.0, y: 5.0 }, &r);
        assert!((got - 5.0).abs() < 1e-9, "got {got}");
    }

    #[test]
    fn test_segment_length_in_rect_parallel_outside_slab() {
        // A vertical segment to the left of the rect is parallel to the
        // left/right slabs and outside them: dx == 0 with q < 0 => 0.
        let r = Rect {
            top: 0.0,
            left: 0.0,
            right: 10.0,
            bottom: 10.0,
        };
        let got =
            segment_length_in_rect(&Point { x: -5.0, y: -5.0 }, &Point { x: -5.0, y: 15.0 }, &r);
        assert_eq!(got, 0.0);
    }
}
