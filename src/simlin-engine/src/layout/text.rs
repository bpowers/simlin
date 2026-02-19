// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::datamodel::view_element::LabelSide;

const LINE_HEIGHT: f64 = 14.0;
const LABEL_PADDING: f64 = 4.0;

/// Per-character advance widths for Roboto-Light at 12pt/72dpi with
/// HintingFull, matching the Praxis reference. Covers ASCII 32..=126;
/// characters outside this range use ROBOTO_LIGHT_AVG.
#[rustfmt::skip]
const ROBOTO_LIGHT_12PT_WIDTHS: [f64; 95] = [
    3.00, 3.00, 3.00, 7.00, 7.00, 9.00, 7.00, 2.00, 4.00, 4.00,
    5.00, 7.00, 2.00, 3.00, 3.00, 5.00, 7.00, 7.00, 7.00, 7.00,
    7.00, 7.00, 7.00, 7.00, 7.00, 7.00, 3.00, 2.00, 6.00, 7.00,
    6.00, 5.00, 11.00, 8.00, 7.00, 8.00, 8.00, 7.00, 7.00, 8.00,
    8.00, 3.00, 7.00, 8.00, 6.00, 10.00, 9.00, 8.00, 7.00, 8.00,
    8.00, 7.00, 7.00, 8.00, 7.00, 11.00, 7.00, 7.00, 7.00, 3.00,
    5.00, 3.00, 5.00, 5.00, 3.00, 6.00, 7.00, 6.00, 7.00, 6.00,
    4.00, 7.00, 7.00, 3.00, 3.00, 6.00, 3.00, 11.00, 7.00, 7.00,
    7.00, 7.00, 4.00, 6.00, 4.00, 7.00, 6.00, 9.00, 6.00, 6.00,
    6.00, 4.00, 3.00, 4.00, 8.00,
];
const ROBOTO_LIGHT_AVG: f64 = 6.12;

/// Estimate text width using per-character Roboto-Light 12pt advance widths.
pub fn estimate_text_width(text: &str) -> f64 {
    text.chars()
        .map(|c| {
            let code = c as u32;
            if (32..=126).contains(&code) {
                ROBOTO_LIGHT_12PT_WIDTHS[(code - 32) as usize]
            } else {
                ROBOTO_LIGHT_AVG
            }
        })
        .sum()
}

/// Estimate the bounding box of a label placed relative to an element.
///
/// Returns `(min_x, min_y, max_x, max_y)` in absolute coordinates.
/// Uses `format_label_with_line_breaks` to determine line count and widths.
pub fn estimate_label_bounds(
    text: &str,
    center_x: f64,
    center_y: f64,
    label_side: LabelSide,
    elem_width: f64,
    elem_height: f64,
) -> (f64, f64, f64, f64) {
    // Labels are already formatted with line breaks when ViewElements are
    // created, so split directly on '\n' rather than re-formatting.
    let lines: Vec<&str> = text.split('\n').collect();
    let max_line_width = lines
        .iter()
        .map(|line| estimate_text_width(line))
        .fold(0.0_f64, f64::max);
    let total_height = lines.len() as f64 * LINE_HEIGHT;
    let half_label_w = max_line_width / 2.0;

    match label_side {
        LabelSide::Bottom => {
            let top = center_y + elem_height / 2.0 + LABEL_PADDING;
            (
                center_x - half_label_w,
                top,
                center_x + half_label_w,
                top + total_height,
            )
        }
        LabelSide::Top => {
            let bottom = center_y - elem_height / 2.0 - LABEL_PADDING;
            (
                center_x - half_label_w,
                bottom - total_height,
                center_x + half_label_w,
                bottom,
            )
        }
        LabelSide::Left => {
            let right = center_x - elem_width / 2.0 - LABEL_PADDING;
            (
                right - max_line_width,
                center_y - total_height / 2.0,
                right,
                center_y + total_height / 2.0,
            )
        }
        LabelSide::Right => {
            let left = center_x + elem_width / 2.0 + LABEL_PADDING;
            (
                left,
                center_y - total_height / 2.0,
                left + max_line_width,
                center_y + total_height / 2.0,
            )
        }
        LabelSide::Center => (
            center_x - half_label_w,
            center_y - total_height / 2.0,
            center_x + half_label_w,
            center_y + total_height / 2.0,
        ),
    }
}

/// Format a label with a single line break at the word boundary closest to the
/// middle of the string.
///
/// Word boundaries are underscores (`_`) and spaces.  If the label contains no
/// word boundaries it is returned unchanged.  The chosen separator character is
/// replaced with a newline, producing exactly two lines.
///
/// This matches the Go `formatLabelWithLineBreaks` behavior: SD variable names
/// are typically snake_case or space-separated, and splitting near the middle
/// produces the most balanced two-line label.
pub fn format_label_with_line_breaks(label: &str) -> String {
    let break_positions: Vec<usize> = label
        .char_indices()
        .filter(|(_, c)| *c == '_' || *c == ' ')
        .map(|(i, _)| i)
        .collect();

    if break_positions.is_empty() {
        return label.to_string();
    }

    let middle = label.len() / 2;
    let mut best_pos = break_positions[0];
    let mut best_distance = (best_pos as isize - middle as isize).unsigned_abs();

    for &pos in &break_positions[1..] {
        let distance = (pos as isize - middle as isize).unsigned_abs();
        if distance < best_distance {
            best_pos = pos;
            best_distance = distance;
        }
    }

    // Replace the chosen separator with a newline.  The separator character
    // is always a single ASCII byte ('_' or ' '), so byte indexing is safe.
    let mut result = String::with_capacity(label.len());
    result.push_str(&label[..best_pos]);
    result.push('\n');
    result.push_str(&label[best_pos + 1..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_text_width() {
        // "hello" = h(7) + e(6) + l(3) + l(3) + o(7) = 26
        assert!((estimate_text_width("hello") - 26.0).abs() < f64::EPSILON);
        assert!((estimate_text_width("") - 0.0).abs() < f64::EPSILON);
        // "a" = 6.0 in Roboto-Light 12pt
        assert!((estimate_text_width("a") - 6.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_narrow_strings_measure_narrower_than_wide() {
        let narrow = estimate_text_width("iii");
        let wide = estimate_text_width("www");
        assert!(
            narrow < wide,
            "narrow 'iii' ({narrow}) should be less than wide 'www' ({wide})"
        );
    }

    #[test]
    fn test_text_width_matches_praxis_reference() {
        // Validate against known Praxis string-level measurements.
        // Char-by-char summing omits kerning, so allow ~2px tolerance.
        assert!((estimate_text_width("population") - 58.0).abs() < 2.0);
        assert!((estimate_text_width("net_population") - 80.0).abs() < 2.0);
        assert!((estimate_text_width("increase_rate") - 68.0).abs() < 2.0);
    }

    #[test]
    fn test_format_label_no_break_needed() {
        assert_eq!(format_label_with_line_breaks("adopters"), "adopters");
        assert_eq!(format_label_with_line_breaks("x"), "x");
    }

    #[test]
    fn test_format_label_line_breaks() {
        assert_eq!(format_label_with_line_breaks("global_rate"), "global\nrate");
        assert_eq!(
            format_label_with_line_breaks("total_population"),
            "total\npopulation"
        );
        assert_eq!(
            format_label_with_line_breaks("net_population_increase_rate"),
            "net_population\nincrease_rate"
        );
        assert_eq!(
            format_label_with_line_breaks("adoption from advertising"),
            "adoption from\nadvertising"
        );
        assert_eq!(
            format_label_with_line_breaks("adoption_from word of mouth"),
            "adoption_from\nword of mouth"
        );
        assert_eq!(
            format_label_with_line_breaks("fractional net increase rate"),
            "fractional net\nincrease rate"
        );
        assert_eq!(format_label_with_line_breaks("a_b_c_d_e_f"), "a_b_c\nd_e_f");
        assert_eq!(
            format_label_with_line_breaks("short_veryverylongword"),
            "short\nveryverylongword"
        );
    }

    #[test]
    fn test_format_label_empty() {
        assert_eq!(format_label_with_line_breaks(""), "");
    }

    #[test]
    fn test_format_label_single_long_word() {
        assert_eq!(
            format_label_with_line_breaks("verylongvariablenamewithoutbreaks"),
            "verylongvariablenamewithoutbreaks"
        );
    }

    #[test]
    fn test_estimate_label_bounds_bottom() {
        let (min_x, min_y, max_x, max_y) =
            estimate_label_bounds("rate", 100.0, 50.0, LabelSide::Bottom, 18.0, 18.0);
        // Label below: min_y should be below element bottom edge
        assert!(min_y > 50.0 + 18.0 / 2.0);
        assert!(max_y > min_y);
        assert!(min_x < 100.0);
        assert!(max_x > 100.0);
    }

    #[test]
    fn test_estimate_label_bounds_right() {
        let (min_x, _min_y, max_x, _max_y) =
            estimate_label_bounds("rate", 100.0, 50.0, LabelSide::Right, 18.0, 18.0);
        // Label right: min_x should be to the right of element right edge
        assert!(min_x > 100.0 + 18.0 / 2.0);
        assert!(max_x > min_x);
    }

    #[test]
    fn test_estimate_label_bounds_long_name() {
        let long_name = "very_long_variable_name_for_testing";
        let (min_x, _min_y, max_x, _max_y) =
            estimate_label_bounds(long_name, 100.0, 50.0, LabelSide::Bottom, 18.0, 18.0);
        let label_width = max_x - min_x;
        assert!(
            label_width > 18.0,
            "label width {} should exceed element width 18",
            label_width
        );
    }

    #[test]
    fn test_estimate_label_bounds_top() {
        let (_min_x, min_y, _max_x, max_y) =
            estimate_label_bounds("x", 100.0, 50.0, LabelSide::Top, 18.0, 18.0);
        // Label above: max_y should be above element top edge
        assert!(max_y < 50.0 - 18.0 / 2.0);
        assert!(min_y < max_y);
    }

    #[test]
    fn test_format_label_non_ascii() {
        // Separators ('_' and ' ') are always ASCII, so byte-indexing at
        // their positions is safe even when the label contains multi-byte
        // UTF-8 characters.
        assert_eq!(
            format_label_with_line_breaks("Bevölkerungs_wachstum"),
            "Bevölkerungs\nwachstum"
        );
        assert_eq!(
            format_label_with_line_breaks("taux de_croissance"),
            "taux de\ncroissance"
        );
    }

    #[test]
    fn test_estimate_label_bounds_already_formatted() {
        // Labels are pre-formatted before being passed to estimate_label_bounds.
        // Passing an already-broken label must not re-break it into 3 lines.
        let pre_formatted = "net_population\nincrease_rate";
        let (_, min_y, _, max_y) =
            estimate_label_bounds(pre_formatted, 100.0, 50.0, LabelSide::Bottom, 18.0, 18.0);
        let height = max_y - min_y;
        let expected_two_lines = 2.0 * LINE_HEIGHT;
        assert!(
            (height - expected_two_lines).abs() < f64::EPSILON,
            "pre-formatted 2-line label should measure 2 lines high ({expected_two_lines}), got {height}",
        );
    }
}
