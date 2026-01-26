// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Settings section parser for Vensim MDL files.
//!
//! The settings section appears after the views/sketch section in MDL files,
//! starting with the `:L<%^E!@` marker (may have `\x7F` after `:L`).
//!
//! Settings are stored as numbered type codes with colon-delimited values:
//! - Type 15: Integration type (comma-separated values, 4th is method code)
//! - Type 22: Unit equivalence strings

use simlin_core::datamodel::{SimMethod, Unit};

/// Parsed settings from an MDL file's settings section.
#[derive(Debug, Default)]
pub struct MdlSettings {
    /// Integration method extracted from type 15 line
    pub integration_method: SimMethod,
    /// Unit equivalences extracted from type 22 lines
    pub unit_equivs: Vec<Unit>,
}

/// Parser for the post-equation section of MDL files (views and settings).
pub struct PostEquationParser<'input> {
    source: &'input str,
}

impl<'input> PostEquationParser<'input> {
    /// Create a new parser from the remaining source after equation parsing.
    ///
    /// The source should be from `EquationReader::remaining()`, which starts
    /// mid-line after the EqEnd marker. We skip to the next line to begin
    /// parsing properly.
    pub fn new(source: &'input str) -> Self {
        // Skip to start of next line (remaining() starts mid-line after EqEnd marker)
        let source = skip_to_next_line(source);
        PostEquationParser { source }
    }

    /// Parse the settings section and return extracted settings.
    pub fn parse_settings(&self) -> MdlSettings {
        let mut settings = MdlSettings::default();

        // Find settings block marker `:L<%^E!@` (may have \x7F after :L)
        let Some(block_start) = self.find_settings_block() else {
            return settings;
        };

        // Parse lines after the block marker
        for line in split_lines(block_start) {
            if line.is_empty() {
                continue;
            }

            // Split on FIRST colon only (paths can have colons like `9:Z:\path\to\file`)
            let Some((type_str, rest)) = line.split_once(':') else {
                continue;
            };
            let type_code = parse_int_like_atoi(type_str);

            match type_code {
                15 => Self::parse_integration_type(rest, &mut settings),
                22 => Self::parse_unit_equivalence(rest, &mut settings),
                _ => {}
            }
        }

        settings
    }

    /// Find the settings block marker `:L<%^E!@` and return the source after it.
    ///
    /// ## Marker Detection Strategy
    ///
    /// We are intentionally more permissive than xmutil:
    /// - xmutil requires `///---\\\` settings section marker to appear first
    /// - xmutil requires DEL character (`\x7F`) between `:L` and `<%^E!@`
    /// - xmutil may require the marker at line start
    ///
    /// We accept:
    /// - Marker anywhere in remaining source (handles files with non-standard view sections)
    /// - With or without the DEL character (handles editor-mangled files)
    /// - Not at line start (handles extra whitespace)
    ///
    /// This permissive approach handles more real-world MDL files while still correctly
    /// identifying the settings block when present.
    fn find_settings_block(&self) -> Option<&'input str> {
        // Look for :L followed by <%^E!@ (with optional \x7F between)
        // Pattern: ":L" + optional_byte + "<%^E!@"
        let bytes = self.source.as_bytes();
        // Minimum marker length is 8 (`:L<%^E!@`), so skip if too short
        if bytes.len() < 8 {
            return None;
        }
        // Check all positions where marker could start (including near end)
        for i in 0..=bytes.len() - 8 {
            if bytes[i] == b':' && bytes.get(i + 1) == Some(&b'L') {
                // Check if followed by <%^E!@ (possibly with \x7F in between)
                let check_pos = if i + 2 < bytes.len() && bytes[i + 2] == 0x7F {
                    i + 3
                } else {
                    i + 2
                };
                if check_pos + 6 <= bytes.len() && &bytes[check_pos..check_pos + 6] == b"<%^E!@" {
                    // Return source starting at next line after marker
                    let marker_end = check_pos + 6;
                    return Some(skip_to_next_line(&self.source[marker_end..]));
                }
            }
        }
        None
    }

    /// Parse type 15: integration type.
    ///
    /// Format: `15:0,0,0,METHOD,0,0` (COMMA-separated, 4th int is method)
    fn parse_integration_type(rest: &str, settings: &mut MdlSettings) {
        let parts: Vec<&str> = rest.split(',').collect();
        if parts.len() >= 4 {
            let method_code = parse_int_like_atoi(parts[3]);
            settings.integration_method = match method_code {
                1 | 5 => SimMethod::RungeKutta4,
                3 | 4 => SimMethod::RungeKutta2,
                _ => SimMethod::Euler,
            };
        }
    }

    /// Parse type 22: unit equivalence.
    ///
    /// Format: `22:$,Dollar,Dollars,$s`
    /// - `$` sets the equation
    /// - First non-`$` token is the name
    /// - Remaining tokens are aliases
    fn parse_unit_equivalence(rest: &str, settings: &mut MdlSettings) {
        // Raw comma tokenization, no trimming (match xmutil exactly)
        let mut name = String::new();
        let mut equation = None;
        let mut aliases = Vec::new();

        for token in rest.split(',') {
            if token.is_empty() {
                continue;
            }

            if token == "$" {
                equation = Some("$".to_string());
            } else if name.is_empty() {
                name = token.to_string();
            } else {
                aliases.push(token.to_string());
            }
        }

        if !name.is_empty() {
            settings.unit_equivs.push(Unit {
                name,
                equation,
                disabled: false,
                aliases,
            });
        }
    }
}

/// Skip to the start of the next line.
///
/// Handles all line ending styles: LF (`\n`), CRLF (`\r\n`), or CR-only (`\r`).
fn skip_to_next_line(s: &str) -> &str {
    // Find the first line ending character
    let end_pos = s.find(['\n', '\r']);
    match end_pos {
        Some(pos) => {
            // Skip past the line ending
            if s[pos..].starts_with("\r\n") {
                &s[pos + 2..]
            } else {
                &s[pos + 1..]
            }
        }
        None => "",
    }
}

/// Parse an integer like C's atoi:
/// - Skip leading whitespace
/// - Handle optional sign (+/-)
/// - Stop at first non-digit character
/// - Return 0 for no digits (empty, whitespace-only, or non-numeric)
fn parse_int_like_atoi(s: &str) -> i32 {
    let s = s.trim_start();
    if s.is_empty() {
        return 0;
    }

    let mut chars = s.chars().peekable();
    let mut sign = 1;

    // Handle optional sign
    if chars.peek() == Some(&'-') {
        sign = -1;
        chars.next();
    } else if chars.peek() == Some(&'+') {
        chars.next();
    }

    // Collect digits
    let mut result: i32 = 0;
    let mut found_digit = false;
    for c in chars {
        if let Some(digit) = c.to_digit(10) {
            found_digit = true;
            result = result.saturating_mul(10).saturating_add(digit as i32);
        } else {
            break;
        }
    }

    if found_digit { result * sign } else { 0 }
}

/// Split string on any line ending: `\n`, `\r\n`, or `\r`.
fn split_lines(s: &str) -> impl Iterator<Item = &str> {
    let mut remaining = s;
    std::iter::from_fn(move || {
        if remaining.is_empty() {
            return None;
        }
        // Find next line ending
        let end = remaining.find(['\n', '\r']).unwrap_or(remaining.len());
        let line = &remaining[..end];

        // Skip the line ending character(s)
        remaining = &remaining[end..];
        if remaining.starts_with("\r\n") {
            remaining = &remaining[2..];
        } else if remaining.starts_with('\n') || remaining.starts_with('\r') {
            remaining = &remaining[1..];
        }

        Some(line)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_integration_type_euler_default() {
        // Type 15 with method code 0 (Euler)
        let source = "\n///---\\\\\\\n:L<%^E!@\n15:0,0,0,0,0,0\n";
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();
        assert_eq!(settings.integration_method, SimMethod::Euler);
    }

    #[test]
    fn test_integration_type_euler_code_2() {
        // Type 15 with method code 2 (also Euler)
        let source = "\n:L<%^E!@\n15:0,0,0,2,0,0\n";
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();
        assert_eq!(settings.integration_method, SimMethod::Euler);
    }

    #[test]
    fn test_integration_type_rk4_code_1() {
        // Type 15 with method code 1 (RK4)
        let source = "\n:L<%^E!@\n15:0,0,0,1,0,0\n";
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();
        assert_eq!(settings.integration_method, SimMethod::RungeKutta4);
    }

    #[test]
    fn test_integration_type_rk4_code_5() {
        // Type 15 with method code 5 (also RK4)
        let source = "\n:L<%^E!@\n15:0,0,0,5,0,0\n";
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();
        assert_eq!(settings.integration_method, SimMethod::RungeKutta4);
    }

    #[test]
    fn test_unit_equivalence_with_dollar() {
        // Unit equivalence: $,Dollar,Dollars,$s
        let source = "\n:L<%^E!@\n22:$,Dollar,Dollars,$s\n";
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();

        assert_eq!(settings.unit_equivs.len(), 1);
        let unit = &settings.unit_equivs[0];
        assert_eq!(unit.name, "Dollar");
        assert_eq!(unit.equation, Some("$".to_string()));
        assert_eq!(unit.aliases, vec!["Dollars", "$s"]);
        assert!(!unit.disabled);
    }

    #[test]
    fn test_unit_equivalence_simple() {
        // Simple unit equivalence without $: Hour,Hours
        let source = "\n:L<%^E!@\n22:Hour,Hours\n";
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();

        assert_eq!(settings.unit_equivs.len(), 1);
        let unit = &settings.unit_equivs[0];
        assert_eq!(unit.name, "Hour");
        assert_eq!(unit.equation, None);
        assert_eq!(unit.aliases, vec!["Hours"]);
    }

    #[test]
    fn test_multiple_unit_equivalences() {
        let source = "\n:L<%^E!@\n22:$,Dollar,Dollars\n22:Hour,Hours,Hr\n";
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();

        assert_eq!(settings.unit_equivs.len(), 2);
        assert_eq!(settings.unit_equivs[0].name, "Dollar");
        assert_eq!(settings.unit_equivs[1].name, "Hour");
    }

    #[test]
    fn test_empty_settings_section() {
        // No settings block marker
        let source = "\nV300\n*View 1\n";
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();

        assert_eq!(settings.integration_method, SimMethod::Euler);
        assert!(settings.unit_equivs.is_empty());
    }

    #[test]
    fn test_settings_with_del_marker() {
        // Settings block with \x7F (DEL character) between :L and <%^E!@
        let source = "\n:L\x7F<%^E!@\n15:0,0,0,1,0,0\n";
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();
        assert_eq!(settings.integration_method, SimMethod::RungeKutta4);
    }

    #[test]
    fn test_type_9_with_colons_in_path() {
        // Type 9 has paths with colons like `9:Z:\path\to\file`
        // This should not break parsing of subsequent lines
        let source = "\n:L<%^E!@\n9:Z:\\CREAF\\dev\\test\n15:0,0,0,1,0,0\n";
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();
        assert_eq!(settings.integration_method, SimMethod::RungeKutta4);
    }

    #[test]
    fn test_crlf_line_endings() {
        // Test with Windows-style CRLF line endings
        let source = "\r\n:L<%^E!@\r\n15:0,0,0,1,0,0\r\n22:Hour,Hours\r\n";
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();
        assert_eq!(settings.integration_method, SimMethod::RungeKutta4);
        assert_eq!(settings.unit_equivs.len(), 1);
    }

    #[test]
    fn test_real_mdl_settings_section() {
        // Based on test_control_vars.mdl
        let source = r#"
///---\\\
:L<%^E!@
1:Current.vdf
9:Z:\CREAF\dev\pysd\tests\test-models\tests\control_vars\Current
15:0,0,0,0,0,0
19:100,0
27:0,
"#;
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();
        assert_eq!(settings.integration_method, SimMethod::Euler);
    }

    #[test]
    fn test_skip_to_next_line() {
        assert_eq!(skip_to_next_line("abc\ndef"), "def");
        assert_eq!(skip_to_next_line("abc"), "");
        assert_eq!(skip_to_next_line("\nabc"), "abc");
        assert_eq!(skip_to_next_line(""), "");
    }

    #[test]
    fn test_skip_to_next_line_cr_only() {
        // Old Mac-style CR-only line endings
        assert_eq!(skip_to_next_line("abc\rdef"), "def");
        assert_eq!(skip_to_next_line("\rabc"), "abc");
    }

    #[test]
    fn test_skip_to_next_line_crlf() {
        // Windows-style CRLF
        assert_eq!(skip_to_next_line("abc\r\ndef"), "def");
        assert_eq!(skip_to_next_line("\r\nabc"), "abc");
    }

    #[test]
    fn test_remaining_starts_mid_line() {
        // Simulates what remaining() returns: starts mid-line after EqEnd marker
        // "\\\---/// Sketch information..." becomes the start point
        let source = " Sketch information - do not modify\nV300\n:L<%^E!@\n15:0,0,0,5,0,0\n";
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();
        assert_eq!(settings.integration_method, SimMethod::RungeKutta4);
    }

    // RK2 integration method tests
    #[test]
    fn test_integration_type_rk2_code_3() {
        // Type 15 with method code 3 (RK2)
        let source = "\n:L<%^E!@\n15:0,0,0,3,0,0\n";
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();
        assert_eq!(settings.integration_method, SimMethod::RungeKutta2);
    }

    #[test]
    fn test_integration_type_rk2_code_4() {
        // Type 15 with method code 4 (also RK2)
        let source = "\n:L<%^E!@\n15:0,0,0,4,0,0\n";
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();
        assert_eq!(settings.integration_method, SimMethod::RungeKutta2);
    }

    // atoi-like parsing tests
    #[test]
    fn test_parse_int_like_atoi_basic() {
        assert_eq!(parse_int_like_atoi("15"), 15);
        assert_eq!(parse_int_like_atoi("0"), 0);
        assert_eq!(parse_int_like_atoi("123"), 123);
    }

    #[test]
    fn test_parse_int_like_atoi_whitespace() {
        // Leading whitespace should be skipped
        assert_eq!(parse_int_like_atoi("  15"), 15);
        assert_eq!(parse_int_like_atoi("\t22"), 22);
        // Trailing non-digits cause parsing to stop
        assert_eq!(parse_int_like_atoi("15 "), 15);
        assert_eq!(parse_int_like_atoi("15abc"), 15);
    }

    #[test]
    fn test_parse_int_like_atoi_signs() {
        assert_eq!(parse_int_like_atoi("-5"), -5);
        assert_eq!(parse_int_like_atoi("+5"), 5);
        assert_eq!(parse_int_like_atoi("  -10"), -10);
    }

    #[test]
    fn test_parse_int_like_atoi_no_digits() {
        // Return 0 when no digits found
        assert_eq!(parse_int_like_atoi(""), 0);
        assert_eq!(parse_int_like_atoi("   "), 0);
        assert_eq!(parse_int_like_atoi("abc"), 0);
        assert_eq!(parse_int_like_atoi("-"), 0);
        assert_eq!(parse_int_like_atoi("+"), 0);
    }

    // split_lines tests
    #[test]
    fn test_split_lines_lf() {
        let lines: Vec<_> = split_lines("a\nb\nc").collect();
        assert_eq!(lines, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_split_lines_crlf() {
        let lines: Vec<_> = split_lines("a\r\nb\r\nc").collect();
        assert_eq!(lines, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_split_lines_cr_only() {
        // Old Mac-style CR-only line endings
        let lines: Vec<_> = split_lines("a\rb\rc").collect();
        assert_eq!(lines, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_split_lines_mixed() {
        // Mixed line endings
        let lines: Vec<_> = split_lines("a\nb\r\nc\rd").collect();
        assert_eq!(lines, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn test_split_lines_empty() {
        let lines: Vec<_> = split_lines("").collect();
        assert!(lines.is_empty());
    }

    #[test]
    fn test_split_lines_no_newline() {
        let lines: Vec<_> = split_lines("single line").collect();
        assert_eq!(lines, vec!["single line"]);
    }

    // CR-only line endings in settings parsing
    #[test]
    fn test_cr_only_line_endings() {
        // Test with old Mac-style CR-only line endings
        let source = "\r:L<%^E!@\r15:0,0,0,3,0,0\r22:Hour,Hours\r";
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();
        assert_eq!(settings.integration_method, SimMethod::RungeKutta2);
        assert_eq!(settings.unit_equivs.len(), 1);
    }

    // Marker at buffer end
    #[test]
    fn test_marker_near_buffer_end() {
        // Settings marker close to the end of the buffer
        let source = "\n:L<%^E!@";
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();
        // Should parse without panic, even if no settings lines follow
        assert_eq!(settings.integration_method, SimMethod::Euler);
    }

    #[test]
    fn test_type_code_with_leading_whitespace() {
        // Type code with leading whitespace should still parse
        let source = "\n:L<%^E!@\n  15:0,0,0,1,0,0\n";
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();
        assert_eq!(settings.integration_method, SimMethod::RungeKutta4);
    }
}
