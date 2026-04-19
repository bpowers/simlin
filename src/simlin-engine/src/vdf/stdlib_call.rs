// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! MDL-side stdlib call parsing and VDF signature generation.
//!
//! Separate from [`super::signatures`], which decodes `#...#` signatures
//! already present in a VDF name table. This module does the inverse
//! direction: parse a user's datamodel equation text like
//! `SMOOTH(x, tau)` into a `StdlibCallInfo`, then generate the
//! Vensim-style signature names that Vensim will have written to the
//! VDF name table when compiling that equation. Used by the
//! model-guided OT-mapping paths in `super::vdf` to align VDF
//! signature names with the user variables that alias them.

/// Information about a stdlib function call extracted from an equation.
pub(super) struct StdlibCallInfo {
    /// The function name (e.g., "SMOOTH", "DELAY1", "SMOOTH3", "DELAY3")
    pub(super) func_name: String,
    /// Raw argument strings from the equation
    pub(super) args: Vec<String>,
}

impl StdlibCallInfo {
    fn args_string(&self) -> String {
        self.args
            .iter()
            .map(|a| a.replace([' ', '_'], ""))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Generate Vensim-style instantiation signature names for VDF ordering.
    ///
    /// Returns (signature, is_stock) pairs. The format matches what Vensim
    /// stores in the VDF name table. Names preserve original case from the
    /// MDL and remove spaces.
    ///
    /// The "I" variants (SMOOTHI, SMOOTH3I, DELAY1I, DELAY3I) take an extra
    /// initial-value argument. The MDL parser normalizes their function names
    /// to the non-I form (e.g., SMOOTHI -> SMTH1), so we distinguish them by
    /// argument count: 2 args = standard, 3 args = "I" variant.
    ///
    /// Observed patterns from VDF dumps:
    /// - SMOOTH: `#SMOOTH(arg1,arg2)#` (stock, 1 entry)
    /// - SMOOTHI: `#SMOOTHI(arg1,arg2,init)#` (stock, 1 entry)
    /// - SMOOTH3: `#SMOOTH3(...)#` (stock=output), `#LV1<SMOOTH3(...)#` (stock),
    ///   `#LV2<SMOOTH3(...)#` (stock), `#DL<SMOOTH3(...)#` (non-stock)
    /// - DELAY1: `#DELAY1(...)#` (non-stock=output), `#LV1<DELAY1(...)#` (stock)
    /// - DELAY3: `#DELAY3(...)#` (non-stock=output), `#LV1<...#` `#LV2<...#`
    ///   `#LV3<...#` (stocks), `#RT1<...#` `#RT2<...#` (non-stock rates),
    ///   `#DL<...#` (non-stock)
    pub(super) fn vensim_signatures(&self) -> Vec<(String, bool)> {
        let args_str = self.args_string();
        let func_upper = self.func_name.to_uppercase();
        let n_args = self.args.len();

        match func_upper.as_str() {
            "SMOOTH" | "SMTH1" if n_args >= 3 => {
                vec![(format!("#SMOOTHI({args_str})#"), true)]
            }
            "SMOOTH" | "SMTH1" => {
                vec![(format!("#SMOOTH({args_str})#"), true)]
            }
            "SMOOTHI" => {
                vec![(format!("#SMOOTHI({args_str})#"), true)]
            }
            "SMOOTH3" | "SMTH3" => {
                let vensim_name = if n_args >= 3 { "SMOOTH3I" } else { "SMOOTH3" };
                let base = format!("{vensim_name}({args_str})");
                vec![
                    (format!("#DL<{base}#"), false),
                    (format!("#LV1<{base}#"), true),
                    (format!("#LV2<{base}#"), true),
                    (format!("#{base}#"), true), // output = 3rd stage stock
                ]
            }
            "DELAY1" | "DELAY" => {
                let vensim_name = if n_args >= 3 { "DELAY1I" } else { "DELAY1" };
                let base = format!("{vensim_name}({args_str})");
                vec![
                    (format!("#{base}#"), false),    // DEL output
                    (format!("#LV1<{base}#"), true), // stock
                ]
            }
            "DELAY3" | "DELAYN" => {
                let vensim_name = if n_args >= 3 { "DELAY3I" } else { "DELAY3" };
                let base = format!("{vensim_name}({args_str})");
                vec![
                    (format!("#{base}#"), false),     // output
                    (format!("#DL<{base}#"), false),  // delay line
                    (format!("#LV1<{base}#"), true),  // stock 1
                    (format!("#LV2<{base}#"), true),  // stock 2
                    (format!("#LV3<{base}#"), true),  // stock 3
                    (format!("#RT1<{base}#"), false), // rate 1
                    (format!("#RT2<{base}#"), false), // rate 2
                ]
            }
            "TREND" => {
                let base = format!("TREND({args_str})");
                vec![
                    (format!("#{base}#"), false),
                    (format!("#LV1<{base}#"), true),
                ]
            }
            _ => {
                vec![(format!("#{func_upper}({args_str})#"), false)]
            }
        }
    }

    /// The VDF signature that a user variable name aliases.
    pub(super) fn output_signature(&self) -> String {
        let args_str = self.args_string();
        let func_upper = self.func_name.to_uppercase();
        let n_args = self.args.len();
        match func_upper.as_str() {
            "SMOOTH" | "SMTH1" if n_args >= 3 => format!("#SMOOTHI({args_str})#"),
            "SMOOTH" | "SMTH1" | "SMOOTHI" => format!("#SMOOTH({args_str})#"),
            "SMOOTH3" | "SMTH3" => {
                let name = if n_args >= 3 { "SMOOTH3I" } else { "SMOOTH3" };
                format!("#{name}({args_str})#")
            }
            "DELAY1" | "DELAY" => {
                let name = if n_args >= 3 { "DELAY1I" } else { "DELAY1" };
                format!("#{name}({args_str})#")
            }
            "DELAY3" | "DELAYN" => {
                let name = if n_args >= 3 { "DELAY3I" } else { "DELAY3" };
                format!("#{name}({args_str})#")
            }
            "TREND" => format!("#TREND({args_str})#"),
            _ => format!("#{func_upper}({args_str})#"),
        }
    }

    /// Whether the user-visible output of this stdlib call is stored in a
    /// stock-backed OT entry.
    #[cfg(test)]
    pub(super) fn output_is_stock(&self) -> bool {
        let output = self.output_signature();
        self.vensim_signatures()
            .into_iter()
            .any(|(sig, is_stock)| is_stock && sig == output)
    }
}

/// Extract stdlib function call information from a datamodel equation.
///
/// Returns None if the equation is not a top-level stdlib call.
pub(super) fn extract_stdlib_call_info(eqn: &crate::datamodel::Equation) -> Option<StdlibCallInfo> {
    let text = match eqn {
        crate::datamodel::Equation::Scalar(s) | crate::datamodel::Equation::ApplyToAll(_, s) => {
            s.as_str()
        }
        _ => return None,
    };

    let trimmed = text.trim();
    let paren_pos = trimmed.find('(')?;
    let func_name = trimmed[..paren_pos].trim();

    let func_lower = func_name.to_lowercase();
    if !crate::builtins::is_stdlib_module_function(&func_lower) {
        return None;
    }

    let after_paren = &trimmed[paren_pos + 1..];
    let close_paren = find_matching_close_paren(after_paren)?;
    let args_str = &after_paren[..close_paren];
    let args = split_top_level_args(args_str);

    Some(StdlibCallInfo {
        func_name: func_name.to_string(),
        args,
    })
}

fn find_matching_close_paren(s: &str) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

fn split_top_level_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                args.push(s[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = s[start..].trim();
    if !last.is_empty() {
        args.push(last.to_string());
    }
    args
}
