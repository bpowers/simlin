// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::common::{Error, ErrorCode, ErrorKind, Result};

/// Trait for resolving external data references during MDL compilation.
///
/// Vensim models can reference external data via GET DIRECT DATA,
/// GET DIRECT CONSTANTS, GET DIRECT LOOKUPS, and GET DIRECT SUBSCRIPT
/// functions. Implementors of this trait provide access to the underlying
/// data files (CSV, Excel, etc.).
///
/// Native builds use `FilesystemDataProvider`; WASM callers can provide
/// pre-loaded data via a custom adapter implementing this trait.
pub trait DataProvider {
    /// Load a time-indexed data series from an external file.
    /// Returns (time, value) pairs suitable for lookup interpolation.
    ///
    /// Arguments follow the Vensim GET DIRECT DATA convention:
    /// - `file`: path to the data file (relative to model directory)
    /// - `tab_or_delimiter`: sheet name (Excel) or delimiter character (CSV)
    /// - `time_col_or_row`: column/row label for the time axis (e.g. "A")
    /// - `cell_label`: cell reference for the start of data (e.g. "B2")
    fn load_data(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        time_col_or_row: &str,
        cell_label: &str,
    ) -> Result<Vec<(f64, f64)>>;

    /// Load a constant value from an external file.
    ///
    /// Arguments follow the Vensim GET DIRECT CONSTANTS convention:
    /// - `file`: path to the data file
    /// - `tab_or_delimiter`: sheet name or delimiter character
    /// - `row_label`: cell reference for the row (e.g. "B2")
    /// - `col_label`: cell reference for the column (e.g. "C2")
    fn load_constant(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        row_label: &str,
        col_label: &str,
    ) -> Result<f64>;

    /// Load a lookup table from an external file.
    /// Returns (x, y) pairs for the lookup function.
    ///
    /// Arguments follow the Vensim GET DIRECT LOOKUPS convention.
    fn load_lookup(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        row_label: &str,
        col_label: &str,
    ) -> Result<Vec<(f64, f64)>>;

    /// Load dimension element names from an external file.
    /// Returns element names as strings.
    ///
    /// Arguments follow the Vensim GET DIRECT SUBSCRIPT convention:
    /// - `file`: path to the data file
    /// - `tab_or_delimiter`: sheet name or delimiter character
    /// - `first_cell`: starting cell reference (e.g. "A2")
    /// - `last_cell`: ending cell reference or column letter (e.g. "A5" or "A")
    fn load_subscript(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        first_cell: &str,
        last_cell: &str,
    ) -> Result<Vec<String>>;
}

/// Default provider that returns errors for all operations.
/// Used when no data files are available (e.g. WASM default).
pub struct NullDataProvider;

impl DataProvider for NullDataProvider {
    fn load_data(
        &self,
        file: &str,
        _tab_or_delimiter: &str,
        _time_col_or_row: &str,
        _cell_label: &str,
    ) -> Result<Vec<(f64, f64)>> {
        Err(data_provider_error(file))
    }

    fn load_constant(
        &self,
        file: &str,
        _tab_or_delimiter: &str,
        _row_label: &str,
        _col_label: &str,
    ) -> Result<f64> {
        Err(data_provider_error(file))
    }

    fn load_lookup(
        &self,
        file: &str,
        _tab_or_delimiter: &str,
        _row_label: &str,
        _col_label: &str,
    ) -> Result<Vec<(f64, f64)>> {
        Err(data_provider_error(file))
    }

    fn load_subscript(
        &self,
        file: &str,
        _tab_or_delimiter: &str,
        _first_cell: &str,
        _last_cell: &str,
    ) -> Result<Vec<String>> {
        Err(data_provider_error(file))
    }
}

fn data_provider_error(file: &str) -> Error {
    Error::new(
        ErrorKind::Import,
        ErrorCode::Generic,
        Some(format!(
            "external data file '{}' referenced but no DataProvider configured",
            file
        )),
    )
}

/// Convert a column letter(s) to a 0-based column index.
/// "A" -> 0, "B" -> 1, ..., "Z" -> 25, "AA" -> 26, etc.
#[cfg(feature = "file_io")]
pub(crate) fn col_index(col: &str) -> Result<usize> {
    if col.is_empty() || !col.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(Error::new(
            ErrorKind::Import,
            ErrorCode::Generic,
            Some(format!(
                "invalid column reference '{}': expected alphabetic characters only",
                col
            )),
        ));
    }
    Ok(col.bytes().fold(0usize, |acc, b| {
        acc * 26 + (b.to_ascii_uppercase() - b'A' + 1) as usize
    }) - 1)
}

/// Parse an A1-style cell reference into 0-based (row, col) indices.
/// "A1" -> (0, 0), "B2" -> (1, 1), "AA10" -> (9, 26)
/// Trailing '*' (Vensim range indicator) is stripped before parsing.
#[cfg(feature = "file_io")]
pub(crate) fn parse_cell_ref(s: &str) -> Result<(usize, usize)> {
    let s = s.trim().trim_end_matches('*');
    let split = s.find(|c: char| c.is_ascii_digit()).ok_or_else(|| {
        Error::new(
            ErrorKind::Import,
            ErrorCode::Generic,
            Some(format!("invalid cell reference '{}': no row number", s)),
        )
    })?;
    if split == 0 {
        return Err(Error::new(
            ErrorKind::Import,
            ErrorCode::Generic,
            Some(format!("invalid cell reference '{}': no column letter", s)),
        ));
    }
    let col = col_index(&s[..split])?;
    let row_1based: usize = s[split..].parse::<usize>().map_err(|_| {
        Error::new(
            ErrorKind::Import,
            ErrorCode::Generic,
            Some(format!("invalid cell reference '{}': bad row number", s)),
        )
    })?;
    if row_1based == 0 {
        return Err(Error::new(
            ErrorKind::Import,
            ErrorCode::Generic,
            Some(format!(
                "invalid cell reference '{}': row numbers are 1-indexed",
                s
            )),
        ));
    }
    Ok((row_1based - 1, col))
}

/// Check if a string is purely column letters (no digits).
#[cfg(feature = "file_io")]
pub(crate) fn is_column_only(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphabetic())
}

#[cfg(feature = "file_io")]
mod csv_provider;
#[cfg(all(feature = "file_io", feature = "ext_data"))]
mod excel_provider;
#[cfg(feature = "file_io")]
pub use csv_provider::FilesystemDataProvider;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_provider_load_data_returns_error_with_filename() {
        let provider = NullDataProvider;
        let result = provider.load_data("test.csv", ",", "A", "B2");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.details.as_ref().unwrap().contains("test.csv"),
            "error should contain the filename"
        );
        assert!(
            err.details
                .as_ref()
                .unwrap()
                .contains("no DataProvider configured"),
        );
    }

    #[test]
    fn null_provider_load_constant_returns_error_with_filename() {
        let provider = NullDataProvider;
        let result = provider.load_constant("data.xlsx", "Sheet1", "B2", "C2");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.details.as_ref().unwrap().contains("data.xlsx"));
    }

    #[test]
    fn null_provider_load_lookup_returns_error_with_filename() {
        let provider = NullDataProvider;
        let result = provider.load_lookup("lookup.csv", ",", "A", "B2");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.details.as_ref().unwrap().contains("lookup.csv"));
    }

    #[test]
    fn null_provider_load_subscript_returns_error_with_filename() {
        let provider = NullDataProvider;
        let result = provider.load_subscript("subs.csv", ",", "A2", "A5");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.details.as_ref().unwrap().contains("subs.csv"));
    }

    #[cfg(feature = "file_io")]
    #[test]
    fn test_col_index() {
        assert_eq!(col_index("A").unwrap(), 0);
        assert_eq!(col_index("B").unwrap(), 1);
        assert_eq!(col_index("Z").unwrap(), 25);
        assert_eq!(col_index("AA").unwrap(), 26);
        assert_eq!(col_index("AB").unwrap(), 27);
        assert_eq!(col_index("a").unwrap(), 0);
        assert_eq!(col_index("b").unwrap(), 1);
    }

    #[cfg(feature = "file_io")]
    #[test]
    fn test_col_index_rejects_invalid_input() {
        assert!(col_index("").is_err());
        assert!(col_index("1").is_err());
        assert!(col_index("A1").is_err());
        assert!(col_index("$A").is_err());
    }

    #[cfg(feature = "file_io")]
    #[test]
    fn test_parse_cell_ref() {
        assert_eq!(parse_cell_ref("A1").unwrap(), (0, 0));
        assert_eq!(parse_cell_ref("B2").unwrap(), (1, 1));
        assert_eq!(parse_cell_ref("C10").unwrap(), (9, 2));
        assert_eq!(parse_cell_ref("AA1").unwrap(), (0, 26));
    }

    #[cfg(feature = "file_io")]
    #[test]
    fn test_parse_cell_ref_strips_star() {
        assert_eq!(parse_cell_ref("B2*").unwrap(), (1, 1));
        assert_eq!(parse_cell_ref("A1*").unwrap(), (0, 0));
    }

    #[cfg(feature = "file_io")]
    #[test]
    fn test_parse_cell_ref_errors() {
        assert!(parse_cell_ref("123").is_err());
        assert!(parse_cell_ref("").is_err());
    }

    #[cfg(feature = "file_io")]
    #[test]
    fn test_parse_cell_ref_row_zero_returns_error() {
        let result = parse_cell_ref("A0");
        assert!(
            result.is_err(),
            "cell reference 'A0' with row 0 should return an error (rows are 1-indexed)"
        );
    }

    #[cfg(feature = "file_io")]
    #[test]
    fn test_is_column_only() {
        assert!(is_column_only("A"));
        assert!(is_column_only("AB"));
        assert!(!is_column_only("A1"));
        assert!(!is_column_only("1"));
        assert!(!is_column_only(""));
    }
}
