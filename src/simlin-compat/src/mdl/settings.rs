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
        for line in block_start.lines() {
            let line = line.trim_end_matches('\r'); // Handle CRLF
            if line.is_empty() {
                continue;
            }

            // Split on FIRST colon only (paths can have colons like `9:Z:\path\to\file`)
            let Some((type_str, rest)) = line.split_once(':') else {
                continue;
            };
            let Ok(type_code) = type_str.parse::<i32>() else {
                continue;
            };

            match type_code {
                15 => Self::parse_integration_type(rest, &mut settings),
                22 => Self::parse_unit_equivalence(rest, &mut settings),
                _ => {}
            }
        }

        settings
    }

    /// Find the settings block marker `:L<%^E!@` and return the source after it.
    fn find_settings_block(&self) -> Option<&'input str> {
        // Look for :L followed by <%^E!@ (with optional \x7F between)
        // Pattern: ":L" + optional_byte + "<%^E!@"
        let bytes = self.source.as_bytes();
        for i in 0..bytes.len().saturating_sub(8) {
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
        if parts.len() >= 4
            && let Ok(method_code) = parts[3].parse::<i32>()
        {
            settings.integration_method = match method_code {
                1 | 5 => SimMethod::RungeKutta4,
                // 3 | 4 would be RK2, but SimMethod doesn't have it; map to Euler
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
fn skip_to_next_line(s: &str) -> &str {
    match s.find('\n') {
        Some(pos) => &s[pos + 1..],
        None => "",
    }
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
    fn test_remaining_starts_mid_line() {
        // Simulates what remaining() returns: starts mid-line after EqEnd marker
        // "\\\---/// Sketch information..." becomes the start point
        let source = " Sketch information - do not modify\nV300\n:L<%^E!@\n15:0,0,0,5,0,0\n";
        let parser = PostEquationParser::new(source);
        let settings = parser.parse_settings();
        assert_eq!(settings.integration_method, SimMethod::RungeKutta4);
    }
}
