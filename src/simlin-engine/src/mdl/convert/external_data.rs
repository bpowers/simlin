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
    /// GET DIRECT CONSTANTS(file, tab, row_or_cell, col)
    Constants {
        file: String,
        tab: String,
        row_or_cell: String,
        col: String,
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
        prefix: String,
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
                    row_or_cell: args[2].clone(),
                    col: args.get(3).cloned().unwrap_or_default(),
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
                    prefix: args.get(4).cloned().unwrap_or_default(),
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
    let mut depth: u32 = 0;

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
                depth = depth.saturating_sub(1);
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

/// Adjust a GET DIRECT call's cell references for a specific array element.
///
/// When a variable is arrayed and its equation uses a GET DIRECT function,
/// each element of the array needs a different cell from the external file.
/// The `element_offsets` slice gives the 0-based position within each
/// **varying** dimension (e.g., `[1, 2]` means second element of dim 0, third
/// of dim 1). Callers must exclude pinned (singleton) dimensions from the
/// offsets; this function uses the last two entries as row/col indices.
///
/// For Constants:
///   - Star pattern (`B2*`): iterate rows, each element bumps the row index
///   - 2D without star: rows = dim 0, cols = dim 1
///   - 2D with star: star dimension iterates over columns
///
/// For Lookups:
///   - Each element reads from a different row (same x-column and starting column)
pub(super) fn adjust_call_for_element(
    call: &GetDirectCall,
    element_offsets: &[usize],
) -> GetDirectCall {
    match call {
        GetDirectCall::Constants {
            file,
            tab,
            row_or_cell,
            col,
        } => {
            let has_star = row_or_cell.contains('*');
            let base_cell = row_or_cell.trim_end_matches('*');

            // Parse the base cell reference to get starting row and column.
            // Two forms: A1-style ("B2") or 4-arg numeric row + col letter
            // ("2" with col="B"). Try A1-style first, then fall back to the
            // 4-arg form where row_or_cell is a plain row number.
            let (base_row, base_col) = match parse_cell_ref_simple(base_cell) {
                Some(rc) => rc,
                None => match parse_row_col_separate(base_cell, col) {
                    Some(rc) => rc,
                    None => return call.clone(),
                },
            };

            // Map varying-dimension offsets to row/col positions in the data file.
            // A GET DIRECT CONSTANTS cell reference addresses a 2D grid, so
            // at most 2 offsets are meaningful. Callers pre-filter to exclude
            // pinned (singleton) dimensions.
            //
            // Without star: first dim -> rows, second dim -> columns
            // With star: first dim -> columns, second dim -> rows (transposed)
            let n = element_offsets.len();
            let (new_row, new_col) = if n == 0 {
                (base_row, base_col)
            } else if n == 1 {
                // Single dimension: iterate down rows
                (base_row + element_offsets[0], base_col)
            } else {
                let first_offset = element_offsets[n - 2];
                let second_offset = element_offsets[n - 1];
                if has_star {
                    // Star reverses the mapping: first dim -> cols, second dim -> rows
                    (base_row + second_offset, base_col + first_offset)
                } else {
                    // Standard mapping: first dim -> rows, second dim -> cols
                    (base_row + first_offset, base_col + second_offset)
                }
            };

            // Reconstruct the cell reference
            let new_cell = format_cell_ref(new_row, new_col);

            GetDirectCall::Constants {
                file: file.clone(),
                tab: tab.clone(),
                row_or_cell: new_cell,
                col: col.clone(),
            }
        }
        GetDirectCall::Lookups {
            file,
            tab,
            x_col,
            y_cell,
        } => {
            // For lookups, each array element reads from a different row
            if element_offsets.is_empty() {
                return call.clone();
            }
            let (base_row, base_col) = match parse_cell_ref_simple(y_cell) {
                Some(rc) => rc,
                None => return call.clone(),
            };
            let new_row = base_row + element_offsets[0];
            let new_cell = format_cell_ref(new_row, base_col);

            GetDirectCall::Lookups {
                file: file.clone(),
                tab: tab.clone(),
                x_col: x_col.clone(),
                y_cell: new_cell,
            }
        }
        // Data and Subscript calls are not adjusted per-element
        _ => call.clone(),
    }
}

/// Parse the 4-arg form where row_or_cell is a plain number and col
/// is a separate column letter reference. Returns 0-based (row, col).
/// ("2", "B") -> Some((1, 1))
fn parse_row_col_separate(row_str: &str, col_str: &str) -> Option<(usize, usize)> {
    let row_1based: usize = row_str.trim().parse().ok()?;
    if row_1based == 0 {
        return None;
    }
    let col_str = col_str.trim();
    if col_str.is_empty() || !col_str.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let col: usize = col_str
        .bytes()
        .fold(0usize, |acc, b| {
            acc * 26 + (b.to_ascii_uppercase() - b'A' + 1) as usize
        })
        .checked_sub(1)?;
    Some((row_1based - 1, col))
}

/// Simple cell reference parser that returns 0-based (row, col).
/// "B2" -> Some((1, 1)), "A1" -> Some((0, 0))
fn parse_cell_ref_simple(s: &str) -> Option<(usize, usize)> {
    let s = s.trim().trim_end_matches('*');
    let split = s.find(|c: char| c.is_ascii_digit())?;
    if split == 0 {
        return None;
    }
    let col_str = &s[..split];
    let row_str = &s[split..];
    let col: usize = col_str
        .bytes()
        .fold(0usize, |acc, b| {
            acc * 26 + (b.to_ascii_uppercase() - b'A' + 1) as usize
        })
        .checked_sub(1)?;
    let row_1based: usize = row_str.parse().ok()?;
    if row_1based == 0 {
        return None;
    }
    Some((row_1based - 1, col))
}

/// Format a 0-based (row, col) back into an A1-style cell reference.
fn format_cell_ref(row: usize, col: usize) -> String {
    let mut col_str = String::new();
    let mut c = col;
    loop {
        col_str.insert(0, (b'A' + (c % 26) as u8) as char);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    format!("{}{}", col_str, row + 1)
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
        GetDirectCall::Constants {
            file,
            tab,
            row_or_cell,
            col,
        } => {
            let file = resolve_file_alias(file, aliases);
            let value = provider.load_constant(&file, tab, row_or_cell, col)?;
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
            prefix,
        } => {
            let file = resolve_file_alias(file, aliases);
            let elements = provider.load_subscript(&file, tab, first_cell, last_cell)?;
            let elements = if prefix.is_empty() {
                elements
            } else {
                elements
                    .into_iter()
                    .map(|element| format!("{prefix}{element}"))
                    .collect()
            };
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
///
/// When `element_offsets` is non-empty, the cell references in the GET DIRECT
/// call are adjusted for the specific array element being resolved.
pub(super) fn try_resolve_data_expr(
    expr_str: &str,
    data_provider: Option<&dyn DataProvider>,
    file_aliases: &HashMap<String, String>,
    element_offsets: &[usize],
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

    let call = if element_offsets.is_empty() {
        call
    } else {
        adjust_call_for_element(&call, element_offsets)
    };

    Some(resolve_get_direct(&call, provider, file_aliases))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_provider::DataProvider;

    struct StubProvider;

    impl DataProvider for StubProvider {
        fn load_data(
            &self,
            _file: &str,
            _tab_or_delimiter: &str,
            _time_col_or_row: &str,
            _cell_label: &str,
        ) -> Result<Vec<(f64, f64)>> {
            Ok(vec![])
        }

        fn load_constant(
            &self,
            _file: &str,
            _tab_or_delimiter: &str,
            _row_label: &str,
            _col_label: &str,
        ) -> Result<f64> {
            Ok(0.0)
        }

        fn load_lookup(
            &self,
            _file: &str,
            _tab_or_delimiter: &str,
            _row_label: &str,
            _col_label: &str,
        ) -> Result<Vec<(f64, f64)>> {
            Ok(vec![])
        }

        fn load_subscript(
            &self,
            _file: &str,
            _tab_or_delimiter: &str,
            _first_cell: &str,
            _last_cell: &str,
        ) -> Result<Vec<String>> {
            Ok(vec!["1".to_string(), "2".to_string()])
        }
    }

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
            GetDirectCall::Constants {
                file,
                tab,
                row_or_cell,
                col,
            } => {
                assert_eq!(file, "data/a.csv");
                assert_eq!(tab, ",");
                assert_eq!(row_or_cell, "B2");
                assert_eq!(col, "");
            }
            _ => panic!("Expected Constants call"),
        }
    }

    #[test]
    fn test_parse_get_direct_constants_4_args() {
        let s = "{GET DIRECT CONSTANTS('data/a.xlsx', 'Sheet1', '2', 'B')}";
        let call = parse_get_direct(s).unwrap();
        match call {
            GetDirectCall::Constants {
                file,
                tab,
                row_or_cell,
                col,
            } => {
                assert_eq!(file, "data/a.xlsx");
                assert_eq!(tab, "Sheet1");
                assert_eq!(row_or_cell, "2");
                assert_eq!(col, "B");
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
                prefix,
            } => {
                assert_eq!(file, "b_subs.csv");
                assert_eq!(tab, ",");
                assert_eq!(first_cell, "A2");
                assert_eq!(last_cell, "A");
                assert_eq!(prefix, "");
            }
            _ => panic!("Expected Subscript call"),
        }
    }

    #[test]
    fn test_try_resolve_subscript_applies_prefix() {
        let expr = "{GET DIRECT SUBSCRIPT('b_subs.csv', ',', 'A2', 'A', 'A')}";
        let result = try_resolve_data_expr(expr, Some(&StubProvider), &HashMap::new(), &[])
            .expect("GET DIRECT expression should be parsed")
            .expect("GET DIRECT expression should resolve");
        match result {
            ResolvedData::Subscript(elements) => {
                assert_eq!(elements, vec!["A1".to_string(), "A2".to_string()]);
            }
            _ => panic!("Expected Subscript result"),
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
        let result = try_resolve_data_expr(expr, None, &HashMap::new(), &[]);
        assert!(result.is_some());
        assert!(result.unwrap().is_err());
    }

    #[test]
    fn test_try_resolve_non_get_direct() {
        let expr = "x + y";
        let result = try_resolve_data_expr(expr, None, &HashMap::new(), &[]);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_get_direct_constants_with_star() {
        // The B2* pattern means "read a row of values starting at B2"
        let s = "{GET DIRECT CONSTANTS('data/b.csv', ',', 'B2*')}";
        let call = parse_get_direct(s).unwrap();
        match call {
            GetDirectCall::Constants {
                file,
                tab,
                row_or_cell,
                col,
            } => {
                assert_eq!(file, "data/b.csv");
                assert_eq!(tab, ",");
                assert_eq!(row_or_cell, "B2*");
                assert_eq!(col, "");
            }
            _ => panic!("Expected Constants call"),
        }
    }

    #[test]
    fn test_parse_cell_ref_simple() {
        assert_eq!(parse_cell_ref_simple("A1"), Some((0, 0)));
        assert_eq!(parse_cell_ref_simple("B2"), Some((1, 1)));
        assert_eq!(parse_cell_ref_simple("C10"), Some((9, 2)));
        assert_eq!(parse_cell_ref_simple("B2*"), Some((1, 1)));
        assert_eq!(parse_cell_ref_simple("A0"), None);
        assert_eq!(parse_cell_ref_simple(""), None);
        assert_eq!(parse_cell_ref_simple("123"), None);
    }

    #[test]
    fn test_format_cell_ref() {
        assert_eq!(format_cell_ref(0, 0), "A1");
        assert_eq!(format_cell_ref(1, 1), "B2");
        assert_eq!(format_cell_ref(9, 2), "C10");
        assert_eq!(format_cell_ref(0, 25), "Z1");
        assert_eq!(format_cell_ref(0, 26), "AA1");
    }

    #[test]
    fn test_adjust_call_constants_1d_star() {
        let call = GetDirectCall::Constants {
            file: "b.csv".to_string(),
            tab: ",".to_string(),
            row_or_cell: "B2*".to_string(),
            col: String::new(),
        };
        let adjusted = adjust_call_for_element(&call, &[2]);
        if let GetDirectCall::Constants { row_or_cell, .. } = adjusted {
            assert_eq!(row_or_cell, "B4");
        } else {
            panic!("Expected Constants");
        }
    }

    #[test]
    fn test_adjust_call_constants_2d_no_star() {
        let call = GetDirectCall::Constants {
            file: "c.csv".to_string(),
            tab: ",".to_string(),
            row_or_cell: "B2".to_string(),
            col: String::new(),
        };
        // [1, 0] = second row, first column from base cell B2
        let adjusted = adjust_call_for_element(&call, &[1, 0]);
        if let GetDirectCall::Constants { row_or_cell, .. } = adjusted {
            assert_eq!(row_or_cell, "B3");
        } else {
            panic!("Expected Constants");
        }
    }

    #[test]
    fn test_adjust_call_constants_2d_star_transposes() {
        let call = GetDirectCall::Constants {
            file: "c.csv".to_string(),
            tab: ",".to_string(),
            row_or_cell: "B2*".to_string(),
            col: String::new(),
        };
        // Star pattern: first dim -> cols, second dim -> rows
        // [1, 2] = second col offset, third row offset
        let adjusted = adjust_call_for_element(&call, &[1, 2]);
        if let GetDirectCall::Constants { row_or_cell, .. } = adjusted {
            assert_eq!(row_or_cell, "C4");
        } else {
            panic!("Expected Constants");
        }
    }

    #[test]
    fn test_adjust_call_constants_varying_dims_only() {
        // Callers must pre-filter element_offsets to only varying dimensions.
        // For a 3-dim variable x[Region, B1, Product] where B1 is pinned,
        // the caller passes only the Region and Product offsets: [2, 1].
        let call = GetDirectCall::Constants {
            file: "c.csv".to_string(),
            tab: ",".to_string(),
            row_or_cell: "B2".to_string(),
            col: String::new(),
        };
        let adjusted = adjust_call_for_element(&call, &[2, 1]);
        if let GetDirectCall::Constants { row_or_cell, .. } = adjusted {
            assert_eq!(row_or_cell, "C4");
        } else {
            panic!("Expected Constants");
        }
    }

    #[test]
    fn test_adjust_call_lookups() {
        let call = GetDirectCall::Lookups {
            file: "lookup.csv".to_string(),
            tab: ",".to_string(),
            x_col: "1".to_string(),
            y_cell: "E2".to_string(),
        };
        let adjusted = adjust_call_for_element(&call, &[2]);
        if let GetDirectCall::Lookups { y_cell, x_col, .. } = adjusted {
            assert_eq!(y_cell, "E4");
            assert_eq!(x_col, "1");
        } else {
            panic!("Expected Lookups");
        }
    }

    #[test]
    fn test_adjust_call_empty_offsets_is_noop() {
        let call = GetDirectCall::Constants {
            file: "a.csv".to_string(),
            tab: ",".to_string(),
            row_or_cell: "B2".to_string(),
            col: String::new(),
        };
        let adjusted = adjust_call_for_element(&call, &[]);
        if let GetDirectCall::Constants { row_or_cell, .. } = adjusted {
            assert_eq!(row_or_cell, "B2");
        } else {
            panic!("Expected Constants");
        }
    }

    #[test]
    fn test_adjust_call_4arg_numeric_row() {
        // 4-arg form: GET DIRECT CONSTANTS(file, tab, '2', 'B')
        // row_or_cell = "2" (row number), col = "B" (column letter)
        let call = GetDirectCall::Constants {
            file: "a.csv".to_string(),
            tab: ",".to_string(),
            row_or_cell: "2".to_string(),
            col: "B".to_string(),
        };
        // Offset [1] means second element -> row 2+1=3, col B
        let adjusted = adjust_call_for_element(&call, &[1]);
        if let GetDirectCall::Constants { row_or_cell, .. } = adjusted {
            assert_eq!(row_or_cell, "B3");
        } else {
            panic!("Expected Constants");
        }
    }

    #[test]
    fn test_adjust_call_4arg_2d() {
        // 4-arg form with 2 varying dimensions
        let call = GetDirectCall::Constants {
            file: "a.csv".to_string(),
            tab: ",".to_string(),
            row_or_cell: "2".to_string(),
            col: "B".to_string(),
        };
        // [1, 2] -> row 2+1=3, col B+2=D
        let adjusted = adjust_call_for_element(&call, &[1, 2]);
        if let GetDirectCall::Constants { row_or_cell, .. } = adjusted {
            assert_eq!(row_or_cell, "D3");
        } else {
            panic!("Expected Constants");
        }
    }

    #[test]
    fn test_parse_row_col_separate() {
        assert_eq!(parse_row_col_separate("2", "B"), Some((1, 1)));
        assert_eq!(parse_row_col_separate("1", "A"), Some((0, 0)));
        assert_eq!(parse_row_col_separate("10", "C"), Some((9, 2)));
        assert_eq!(parse_row_col_separate("0", "A"), None);
        assert_eq!(parse_row_col_separate("abc", "A"), None);
        assert_eq!(parse_row_col_separate("2", ""), None);
    }
}
