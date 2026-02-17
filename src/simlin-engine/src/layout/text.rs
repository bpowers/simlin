// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/// Estimate text width based on character count.
///
/// Uses a simple heuristic: each character is approximately 7 pixels wide.
/// This is a rough approximation suitable for layout planning, not precise
/// text rendering.
pub fn estimate_text_width(text: &str) -> f64 {
    text.chars().count() as f64 * 7.0
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
        assert!((estimate_text_width("hello") - 35.0).abs() < f64::EPSILON);
        assert!((estimate_text_width("") - 0.0).abs() < f64::EPSILON);
        assert!((estimate_text_width("a") - 7.0).abs() < f64::EPSILON);
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
}
