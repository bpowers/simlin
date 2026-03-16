// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Stdlib call analysis and helper functions for VDF name-to-OT mapping.

#[cfg(feature = "file_io")]
use std::collections::HashMap;

#[cfg(feature = "file_io")]
use super::{Canonical, Ident, Variable};

/// Information about a stdlib function call extracted from an equation.
#[cfg(feature = "file_io")]
pub(crate) struct StdlibCallInfo {
    /// The function name (e.g., "SMOOTH", "DELAY1", "SMOOTH3", "DELAY3")
    pub(crate) func_name: String,
    /// Raw argument strings from the equation
    pub(crate) args: Vec<String>,
}

#[cfg(feature = "file_io")]
impl StdlibCallInfo {
    pub(crate) fn args_string(&self) -> String {
        self.args
            .iter()
            .map(|a| a.replace([' ', '_'], ""))
            .collect::<Vec<_>>()
            .join(",")
    }

    pub(crate) fn compiled_stdlib_module_name(&self) -> Option<&'static str> {
        let func_upper = self.func_name.to_uppercase();
        let n_args = self.args.len();
        match func_upper.as_str() {
            "SMOOTH" | "SMOOTHI" | "SMTH1" => Some("smth1"),
            "SMOOTH3" | "SMTH3" => Some("smth3"),
            "DELAY" | "DELAY1" => Some("delay1"),
            "DELAY3" => Some("delay3"),
            "DELAYN" => match n_args {
                0..=2 => Some("delay3"),
                _ => Some("delay3"),
            },
            "SMTHN" => match n_args {
                0..=2 => Some("smth3"),
                _ => Some("smth3"),
            },
            "TREND" => Some("trend"),
            "NPV" => Some("npv"),
            _ => None,
        }
    }

    /// Generate Vensim-style instantiation signature names for VDF ordering.
    ///
    /// Returns (signature, is_stock) pairs. The format matches what Vensim
    /// stores in the VDF name table. Names preserve original case from the
    /// MDL and remove spaces.
    ///
    /// The "I" variants (SMOOTHI, SMOOTH3I, DELAY1I, DELAY3I) take an extra
    /// initial-value argument. The MDL parser normalizes their function names
    /// to the non-I form (e.g., SMOOTHI → SMTH1), so we distinguish them by
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
    pub(crate) fn vensim_signatures(&self) -> Vec<(String, bool)> {
        // Vensim VDF signatures have no spaces or underscores between words.
        // The MDL parser may have already canonicalized spaces to underscores,
        // so strip both.
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
    ///
    /// For SMOOTH, the user var shares OT with the `#SMOOTH(...)#` stock.
    /// For DELAY, the user var shares OT with the `#DELAY1(...)#` output.
    /// For SMOOTH3, the user var shares OT with the `#SMOOTH3(...)#` output.
    pub(crate) fn output_signature(&self) -> String {
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

    pub(crate) fn member_vdf_targets(&self) -> Vec<(&'static str, String)> {
        let args_str = self.args_string();
        let func_upper = self.func_name.to_uppercase();
        let n_args = self.args.len();
        match func_upper.as_str() {
            "SMOOTH" | "SMTH1" if n_args >= 3 => {
                vec![("output", format!("#SMOOTHI({args_str})#"))]
            }
            "SMOOTH" | "SMTH1" | "SMOOTHI" => {
                vec![("output", format!("#SMOOTH({args_str})#"))]
            }
            "SMOOTH3" | "SMTH3" => {
                let name = if n_args >= 3 { "SMOOTH3I" } else { "SMOOTH3" };
                let base = format!("{name}({args_str})");
                vec![
                    ("output", format!("#{base}#")),
                    ("stock_1", format!("#LV1<{base}#")),
                    ("stock_2", format!("#LV2<{base}#")),
                ]
            }
            "DELAY1" | "DELAY" => {
                let name = if n_args >= 3 { "DELAY1I" } else { "DELAY1" };
                let base = format!("{name}({args_str})");
                vec![
                    ("output", format!("#{base}#")),
                    ("stock", format!("#LV1<{base}#")),
                ]
            }
            "DELAY3" | "DELAYN" => {
                let name = if n_args >= 3 { "DELAY3I" } else { "DELAY3" };
                let base = format!("{name}({args_str})");
                vec![
                    ("output", format!("#{base}#")),
                    ("stock", format!("#LV1<{base}#")),
                    ("stock_2", format!("#LV2<{base}#")),
                    ("stock_3", format!("#LV3<{base}#")),
                    ("flow_1", format!("#RT1<{base}#")),
                    ("flow_2", format!("#RT2<{base}#")),
                ]
            }
            "TREND" => {
                let base = format!("TREND({args_str})");
                vec![
                    ("output", format!("#{base}#")),
                    ("stock", format!("#LV1<{base}#")),
                ]
            }
            "NPV" => vec![("output", format!("#NPV({args_str})#"))],
            _ => Vec::new(),
        }
    }

    /// Whether the user-visible output of this stdlib call is stored in a
    /// stock-backed OT entry.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn output_is_stock(&self) -> bool {
        let output = self.output_signature();
        self.vensim_signatures()
            .into_iter()
            .any(|(sig, is_stock)| is_stock && sig == output)
    }
}

/// Names of stdlib module internal variables that DO consume OT entries.
/// These are the expansion variables that Vensim saves alongside the
/// `#`-prefixed instantiation signatures. LV1/LV2/LV3/ST are stock-backed;
/// DEL/DL/RT1/RT2 are non-stock.
#[cfg(feature = "file_io")]
pub(crate) const STDLIB_PARTICIPANT_HELPERS: [&str; 8] =
    ["DEL", "LV1", "LV2", "LV3", "ST", "RT1", "RT2", "DL"];

/// Whether a stdlib participant helper name is stock-backed in Vensim's
/// expansion model. LV (level) and ST (state) variables are stocks;
/// DEL, DL (delay line), RT (rate) are non-stocks.
#[cfg(feature = "file_io")]
pub(crate) fn is_stdlib_helper_stock(name: &str) -> bool {
    matches!(name, "LV1" | "LV2" | "LV3" | "ST")
}

/// Check if a VDF name table entry is metadata rather than a variable.
/// Metadata entries include unit annotations ("-months"), model/group
/// identifiers (".Control"), and stdlib module names that don't have
/// their own OT slots.
///
/// Participant helper names (DEL, LV1, etc.) are NOT considered metadata
/// because they DO consume OT entries. Use [`STDLIB_PARTICIPANT_HELPERS`]
/// for those.
#[cfg(feature = "file_io")]
pub(crate) fn is_vdf_metadata_entry(name: &str) -> bool {
    // Pure numeric labels show up in some name tables as structural markers,
    // not user variables. Treat them as metadata so they don't steal OT slots
    // from real model variables.
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    // Unit annotations: "-months", "-dmnl", "-$", "-1"
    if name.starts_with('-') {
        return true;
    }
    // Model/group identifiers: ".mark2", ".Control"
    if name.starts_with('.') {
        return true;
    }
    // Vensim metadata tags
    if name.starts_with(':') {
        return true;
    }
    // Quoted dimension/subscript names: "\"Absorption Land (GHA)\""
    if name.starts_with('"') {
        return true;
    }
    // Stdlib module function names and IO variable names. These appear
    // in the name table but don't have independent OT slots.
    // Note: participant helpers (DEL, LV1, etc.) are intentionally NOT
    // listed here because they DO consume OT entries.
    if matches!(
        name,
        "IN" | "INI"
            | "OUTPUT"
            | "SMOOTH"
            | "SMOOTHI"
            | "SMOOTH3"
            | "SMOOTH3I"
            | "DELAY1"
            | "DELAY1I"
            | "DELAY3"
            | "DELAY3I"
            | "TREND"
            | "NPV"
    ) {
        return true;
    }
    // Vensim builtins that appear in the name table as function
    // references but aren't captured by VENSIM_BUILTINS.
    let lower = name.replace([' ', '_'], "").to_lowercase();
    matches!(lower.as_str(), "ifthenelse" | "withlookup" | "lookup")
}

/// Heuristic for visible lookup/table definitions that appear in the VDF name
/// table but do not always consume OT slots. These names are kept when section
/// 6 references their slot and dropped otherwise.
#[cfg(feature = "file_io")]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn is_probable_lookup_table_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains(" lookup") || lower.contains(" table")
}

/// Normalize a VDF name for comparison: lowercase, strip spaces and underscores.
/// VDF names use spaces ("Agricultural Inputs"), MDL idents use underscores
/// ("agricultural_inputs"), and #-prefixed signatures use neither.
#[cfg(feature = "file_io")]
pub(crate) fn normalize_vdf_name(name: &str) -> String {
    name.replace([' ', '_'], "").to_lowercase()
}

#[cfg(feature = "file_io")]
pub(crate) fn collect_direct_stdlib_calls(
    model: &crate::datamodel::Model,
) -> HashMap<String, StdlibCallInfo> {
    let mut calls = HashMap::new();
    for var in &model.variables {
        let (ident, equation) = match var {
            crate::datamodel::Variable::Aux(a) => (&a.ident, &a.equation),
            crate::datamodel::Variable::Flow(f) => (&f.ident, &f.equation),
            _ => continue,
        };
        if let Some(info) = extract_stdlib_call_info(equation) {
            calls.insert(crate::common::canonicalize(ident).into_owned(), info);
        }
    }
    calls
}

#[cfg(feature = "file_io")]
pub(crate) fn parse_implicit_stdlib_module_ident(name: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = name.split('\u{205A}').collect();
    if parts.len() < 4 || parts.first().copied() != Some("$") {
        return None;
    }
    let parent = parts.get(1)?.to_string();
    let func = parts.get(3)?.to_lowercase();
    Some((parent, func))
}

#[cfg(feature = "file_io")]
pub(crate) fn prefixed_ident(prefix: Option<&str>, ident: &str) -> String {
    if let Some(prefix) = prefix {
        format!("{prefix}.{ident}")
    } else {
        ident.to_string()
    }
}

#[cfg(feature = "file_io")]
pub(crate) fn collect_compiled_alias_edges(
    project: &crate::Project,
    datamodel_models: &HashMap<&str, &crate::datamodel::Model>,
    model_name: &str,
    prefix: Option<&str>,
) -> Vec<(Ident<Canonical>, String)> {
    let compiled_model =
        std::sync::Arc::clone(&project.models[&*crate::common::canonicalize(model_name)]);
    let direct_calls = datamodel_models
        .get(model_name)
        .map(|m| collect_direct_stdlib_calls(m))
        .unwrap_or_default();

    let mut out = Vec::new();
    let mut var_names: Vec<&str> = compiled_model
        .variables
        .keys()
        .map(|s| s.as_str())
        .collect();
    var_names.sort_unstable();

    for ident in var_names {
        let full_ident = prefixed_ident(prefix, ident);
        let var = &compiled_model.variables[&*crate::common::canonicalize(ident)];
        let Variable::Module {
            model_name: submodel_name,
            inputs,
            ..
        } = var
        else {
            continue;
        };

        for input in inputs {
            let member_name = format!("{full_ident}.{}", input.dst.to_source_repr());
            let source_name = prefixed_ident(prefix, &input.src.to_source_repr().to_string());
            out.push((
                Ident::<Canonical>::from_unchecked(member_name),
                normalize_vdf_name(&source_name),
            ));
        }

        if submodel_name.as_str().starts_with("stdlib\u{205A}")
            && let Some((parent_name, module_func)) = parse_implicit_stdlib_module_ident(ident)
            && let Some(info) = direct_calls.get(&parent_name)
            && info
                .compiled_stdlib_module_name()
                .is_some_and(|name| name == module_func)
        {
            for (member, target) in info.member_vdf_targets() {
                out.push((
                    Ident::<Canonical>::from_unchecked(format!("{full_ident}.{member}")),
                    normalize_vdf_name(&target),
                ));
            }
            continue;
        }

        out.extend(collect_compiled_alias_edges(
            project,
            datamodel_models,
            submodel_name.as_str(),
            Some(&full_ident),
        ));
    }

    out
}

/// Extract stdlib function call information from a datamodel equation.
///
/// Returns None if the equation is not a top-level stdlib call.
#[cfg(feature = "file_io")]
pub(crate) fn extract_stdlib_call_info(eqn: &crate::datamodel::Equation) -> Option<StdlibCallInfo> {
    let text = match eqn {
        crate::datamodel::Equation::Scalar(s) | crate::datamodel::Equation::ApplyToAll(_, s) => {
            s.as_str()
        }
        _ => return None,
    };

    // Parse just enough to extract function name and arguments
    let trimmed = text.trim();

    // Find the function name (everything before the first '(')
    let paren_pos = trimmed.find('(')?;
    let func_name = trimmed[..paren_pos].trim();

    // Check if it's a stdlib function
    let func_lower = func_name.to_lowercase();
    if !crate::builtins::is_stdlib_module_function(&func_lower) {
        return None;
    }

    // Extract arguments by finding matching parens
    let after_paren = &trimmed[paren_pos + 1..];
    let close_paren = find_matching_close_paren(after_paren)?;
    let args_str = &after_paren[..close_paren];

    // Split by commas at the top level (respecting nested parens)
    let args = split_top_level_args(args_str);

    Some(StdlibCallInfo {
        func_name: func_name.to_string(),
        args,
    })
}

/// Find the position of the matching close parenthesis.
#[cfg(feature = "file_io")]
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

/// Split a string by commas at the top level (not inside parentheses).
#[cfg(feature = "file_io")]
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
