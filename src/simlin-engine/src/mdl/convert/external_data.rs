// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! External data resolution for GET DIRECT functions during MDL conversion.
//!
//! The normalizer produces opaque strings like `{GET DIRECT DATA('file','tab','A','B2')}`
//! for GET DIRECT function calls. This module parses those strings and calls the
//! DataProvider to resolve them into lookup tables, constants, or subscript elements.

use std::collections::HashMap;

use crate::common::{Error, ErrorCode, ErrorKind, Result};
use crate::data_provider::DataProvider;
use crate::datamodel::{GraphicalFunction, GraphicalFunctionKind, GraphicalFunctionScale};

/// Parsed GET DIRECT function call.
#[derive(Clone)]
pub(super) enum GetDirectCall {
    /// GET DIRECT DATA(file, tab, time_col, data_cell)
    Data {
        file: String,
        tab: String,
        time_col: String,
        data_cell: String,
    },
    /// GET DIRECT CONSTANTS(file, tab, cell)
    Constants {
        file: String,
        tab: String,
        cell: String,
    },
    /// GET DIRECT LOOKUPS(file, tab, x_col, y_cell)
    Lookups {
        file: String,
        tab: String,
        x_col: String,
        y_cell: String,
    },
    /// GET DIRECT SUBSCRIPT(file, tab, first_cell, last_cell, prefix)
    Subscript {
        file: String,
        tab: String,
        first_cell: String,
        last_cell: String,
    },
}

/// Result of resolving external data via DataProvider.
pub(super) enum ResolvedData {
    /// Lookup table: equation becomes "TIME" with a graphical function
    Lookup(String, GraphicalFunction),
    /// Constant value: equation becomes the numeric literal
    Constant(f64),
    /// Subscript elements: list of dimension element names
    Subscript(Vec<String>),
}

/// Try to parse an opaque GET DIRECT string produced by the normalizer.
/// Returns None if the string doesn't match the expected pattern.
pub(super) fn parse_get_direct(s: &str) -> Option<GetDirectCall> {
    let s = s.trim();

    // The normalizer wraps these in braces: {GET DIRECT DATA('file','tab','A','B2')}
    let inner = s.strip_prefix('{')?.strip_suffix('}')?;

    // Split function name from arguments at the opening paren
    let paren_pos = inner.find('(')?;
    let func_name = inner[..paren_pos].trim();
    let args_str = inner[paren_pos + 1..].strip_suffix(')')?.trim();

    // Parse arguments: split on commas, strip quotes and whitespace
    let args = parse_quoted_args(args_str);

    let canon = func_name
        .to_ascii_lowercase()
        .replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    match canon.as_str() {
        "get direct data" => {
            if args.len() >= 4 {
                Some(GetDirectCall::Data {
                    file: args[0].clone(),
                    tab: args[1].clone(),
                    time_col: args[2].clone(),
                    data_cell: args[3].clone(),
                })
            } else {
                None
            }
        }
        "get direct constants" => {
            if args.len() >= 3 {
                Some(GetDirectCall::Constants {
                    file: args[0].clone(),
                    tab: args[1].clone(),
                    cell: args[2].clone(),
                })
            } else {
                None
            }
        }
        "get direct lookups" => {
            if args.len() >= 4 {
                Some(GetDirectCall::Lookups {
                    file: args[0].clone(),
                    tab: args[1].clone(),
                    x_col: args[2].clone(),
                    y_cell: args[3].clone(),
                })
            } else {
                None
            }
        }
        "get direct subscript" => {
            if args.len() >= 4 {
                Some(GetDirectCall::Subscript {
                    file: args[0].clone(),
                    tab: args[1].clone(),
                    first_cell: args[2].clone(),
                    last_cell: args[3].clone(),
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Parse comma-separated arguments, stripping single quotes and whitespace.
fn parse_quoted_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut depth = 0;

    for ch in s.chars() {
        match ch {
            '\'' if depth == 0 => {
                in_quotes = !in_quotes;
            }
            '(' if !in_quotes => {
                depth += 1;
                current.push(ch);
            }
            ')' if !in_quotes => {
                depth -= 1;
                current.push(ch);
            }
            ',' if !in_quotes && depth == 0 => {
                args.push(current.trim().to_string());
                current = String::new();
            }
            _ => {
                current.push(ch);
            }
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() || !args.is_empty() {
        args.push(trimmed);
    }
    args
}

/// Resolve a file reference, substituting aliases from the settings section.
/// For example, `?data` might map to `data.xlsx` via a type 30 setting.
pub(super) fn resolve_file_alias(file: &str, aliases: &HashMap<String, String>) -> String {
    // Check if the file reference matches an alias exactly
    if let Some(resolved) = aliases.get(file) {
        return resolved.clone();
    }
    file.to_string()
}

/// Resolve a GET DIRECT call using a DataProvider.
pub(super) fn resolve_get_direct(
    call: &GetDirectCall,
    provider: &dyn DataProvider,
    aliases: &HashMap<String, String>,
) -> Result<ResolvedData> {
    match call {
        GetDirectCall::Data {
            file,
            tab,
            time_col,
            data_cell,
        } => {
            let file = resolve_file_alias(file, aliases);
            let pairs = provider.load_data(&file, tab, time_col, data_cell)?;
            let gf = pairs_to_graphical_function(&pairs);
            Ok(ResolvedData::Lookup("TIME".to_string(), gf))
        }
        GetDirectCall::Constants { file, tab, cell } => {
            let file = resolve_file_alias(file, aliases);
            let value = provider.load_constant(&file, tab, cell, "")?;
            Ok(ResolvedData::Constant(value))
        }
        GetDirectCall::Lookups {
            file,
            tab,
            x_col,
            y_cell,
        } => {
            let file = resolve_file_alias(file, aliases);
            let pairs = provider.load_lookup(&file, tab, x_col, y_cell)?;
            let gf = pairs_to_graphical_function(&pairs);
            Ok(ResolvedData::Lookup("TIME".to_string(), gf))
        }
        GetDirectCall::Subscript {
            file,
            tab,
            first_cell,
            last_cell,
        } => {
            let file = resolve_file_alias(file, aliases);
            let elements = provider.load_subscript(&file, tab, first_cell, last_cell)?;
            Ok(ResolvedData::Subscript(elements))
        }
    }
}

/// Convert (x, y) pairs to a GraphicalFunction lookup table.
fn pairs_to_graphical_function(pairs: &[(f64, f64)]) -> GraphicalFunction {
    if pairs.is_empty() {
        return GraphicalFunction {
            kind: GraphicalFunctionKind::Continuous,
            x_points: Some(vec![0.0, 1.0]),
            y_points: vec![0.0, 0.0],
            x_scale: GraphicalFunctionScale { min: 0.0, max: 1.0 },
            y_scale: GraphicalFunctionScale { min: 0.0, max: 0.0 },
        };
    }

    let x_points: Vec<f64> = pairs.iter().map(|(x, _)| *x).collect();
    let y_points: Vec<f64> = pairs.iter().map(|(_, y)| *y).collect();

    let x_min = x_points.iter().cloned().fold(f64::INFINITY, f64::min);
    let x_max = x_points.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let y_min = y_points.iter().cloned().fold(f64::INFINITY, f64::min);
    let y_max = y_points.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    GraphicalFunction {
        kind: GraphicalFunctionKind::Continuous,
        x_points: Some(x_points),
        y_points,
        x_scale: GraphicalFunctionScale {
            min: x_min,
            max: x_max,
        },
        y_scale: GraphicalFunctionScale {
            min: y_min,
            max: y_max,
        },
    }
}

/// Check if an expression string is a GET DIRECT reference (opaque normalizer output).
pub(super) fn is_get_direct_ref(expr_str: &str) -> bool {
    let trimmed = expr_str.trim();
    trimmed.starts_with("{GET DIRECT")
}

/// Try to resolve a GET DIRECT reference from an expression string.
/// Returns None if the string isn't a GET DIRECT reference or if no DataProvider
/// is configured.
pub(super) fn try_resolve_data_expr(
    expr_str: &str,
    data_provider: Option<&dyn DataProvider>,
    file_aliases: &HashMap<String, String>,
) -> Option<Result<ResolvedData>> {
    if !is_get_direct_ref(expr_str) {
        return None;
    }

    let call = parse_get_direct(expr_str)?;

    let provider = match data_provider {
        Some(p) => p,
        None => {
            // Extract filename for better error message
            let file = match &call {
                GetDirectCall::Data { file, .. }
                | GetDirectCall::Constants { file, .. }
                | GetDirectCall::Lookups { file, .. }
                | GetDirectCall::Subscript { file, .. } => file.clone(),
            };
            return Some(Err(Error::new(
                ErrorKind::Import,
                ErrorCode::Generic,
                Some(format!(
                    "external data file '{}' referenced but no DataProvider configured",
                    file
                )),
            )));
        }
    };

    Some(resolve_get_direct(&call, provider, file_aliases))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_get_direct_data() {
        let s = "{GET DIRECT DATA('?data', 'A Data', 'A', 'B2')}";
        let call = parse_get_direct(s).unwrap();
        match call {
            GetDirectCall::Data {
                file,
                tab,
                time_col,
                data_cell,
            } => {
                assert_eq!(file, "?data");
                assert_eq!(tab, "A Data");
                assert_eq!(time_col, "A");
                assert_eq!(data_cell, "B2");
            }
            _ => panic!("Expected Data call"),
        }
    }

    #[test]
    fn test_parse_get_direct_constants() {
        let s = "{GET DIRECT CONSTANTS('data/a.csv', ',', 'B2')}";
        let call = parse_get_direct(s).unwrap();
        match call {
            GetDirectCall::Constants { file, tab, cell } => {
                assert_eq!(file, "data/a.csv");
                assert_eq!(tab, ",");
                assert_eq!(cell, "B2");
            }
            _ => panic!("Expected Constants call"),
        }
    }

    #[test]
    fn test_parse_get_direct_lookups() {
        let s = "{GET DIRECT LOOKUPS('lookup_data.csv', ',', '1', 'E2')}";
        let call = parse_get_direct(s).unwrap();
        match call {
            GetDirectCall::Lookups {
                file,
                tab,
                x_col,
                y_cell,
            } => {
                assert_eq!(file, "lookup_data.csv");
                assert_eq!(tab, ",");
                assert_eq!(x_col, "1");
                assert_eq!(y_cell, "E2");
            }
            _ => panic!("Expected Lookups call"),
        }
    }

    #[test]
    fn test_parse_get_direct_subscript() {
        let s = "{GET DIRECT SUBSCRIPT('b_subs.csv', ',', 'A2', 'A', '')}";
        let call = parse_get_direct(s).unwrap();
        match call {
            GetDirectCall::Subscript {
                file,
                tab,
                first_cell,
                last_cell,
            } => {
                assert_eq!(file, "b_subs.csv");
                assert_eq!(tab, ",");
                assert_eq!(first_cell, "A2");
                assert_eq!(last_cell, "A");
            }
            _ => panic!("Expected Subscript call"),
        }
    }

    #[test]
    fn test_parse_get_direct_with_spaces_in_args() {
        let s = "{GET DIRECT DATA( '?data' , 'A Data' , 'A' , 'B2' )}";
        let call = parse_get_direct(s);
        assert!(call.is_some(), "Should parse even with extra spaces");
    }

    #[test]
    fn test_parse_get_direct_non_matching() {
        assert!(parse_get_direct("regular_variable").is_none());
        assert!(parse_get_direct("{GET XLS('file')}").is_none());
        assert!(parse_get_direct("").is_none());
    }

    #[test]
    fn test_resolve_file_alias() {
        let mut aliases = HashMap::new();
        aliases.insert("?data".to_string(), "data.xlsx".to_string());

        assert_eq!(resolve_file_alias("?data", &aliases), "data.xlsx");
        assert_eq!(resolve_file_alias("other.csv", &aliases), "other.csv");
    }

    #[test]
    fn test_is_get_direct_ref() {
        assert!(is_get_direct_ref(
            "{GET DIRECT DATA('file','tab','A','B2')}"
        ));
        assert!(is_get_direct_ref(
            "{GET DIRECT CONSTANTS('file','tab','B2')}"
        ));
        assert!(is_get_direct_ref(
            "{GET DIRECT LOOKUPS('file','tab','A','B2')}"
        ));
        assert!(is_get_direct_ref(
            "{GET DIRECT SUBSCRIPT('file','tab','A2','A','')}"
        ));
        assert!(!is_get_direct_ref("regular_expr"));
        assert!(!is_get_direct_ref("{GET XLS('file')}"));
    }

    #[test]
    fn test_pairs_to_graphical_function() {
        let pairs = vec![(2000.0, 10.0), (2010.0, 20.0), (2020.0, 30.0)];
        let gf = pairs_to_graphical_function(&pairs);
        assert_eq!(gf.x_points, Some(vec![2000.0, 2010.0, 2020.0]));
        assert_eq!(gf.y_points, vec![10.0, 20.0, 30.0]);
        assert_eq!(gf.x_scale.min, 2000.0);
        assert_eq!(gf.x_scale.max, 2020.0);
        assert_eq!(gf.y_scale.min, 10.0);
        assert_eq!(gf.y_scale.max, 30.0);
    }

    #[test]
    fn test_pairs_to_graphical_function_empty() {
        let pairs: Vec<(f64, f64)> = vec![];
        let gf = pairs_to_graphical_function(&pairs);
        assert_eq!(gf.x_points, Some(vec![0.0, 1.0]));
        assert_eq!(gf.y_points, vec![0.0, 0.0]);
    }

    #[test]
    fn test_try_resolve_without_provider() {
        let expr = "{GET DIRECT DATA('file.csv', ',', 'A', 'B2')}";
        let result = try_resolve_data_expr(expr, None, &HashMap::new());
        assert!(result.is_some());
        assert!(result.unwrap().is_err());
    }

    #[test]
    fn test_try_resolve_non_get_direct() {
        let expr = "x + y";
        let result = try_resolve_data_expr(expr, None, &HashMap::new());
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_get_direct_constants_with_star() {
        // The B2* pattern means "read a row of values starting at B2"
        let s = "{GET DIRECT CONSTANTS('data/b.csv', ',', 'B2*')}";
        let call = parse_get_direct(s).unwrap();
        match call {
            GetDirectCall::Constants { file, tab, cell } => {
                assert_eq!(file, "data/b.csv");
                assert_eq!(tab, ",");
                assert_eq!(cell, "B2*");
            }
            _ => panic!("Expected Constants call"),
        }
    }
}
