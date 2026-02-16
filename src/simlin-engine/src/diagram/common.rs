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

/// Format a floating point number to match JavaScript's Number.toString() behavior.
/// Key differences: no trailing .0 for integers, minimal decimal places.
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

    // For integers, JS outputs no decimal point
    if n == n.trunc() && n.abs() < 1e21 {
        return format!("{}", n as i64);
    }

    format!("{}", n)
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
}
