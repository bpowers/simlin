// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::path::PathBuf;

use crate::common::{Error, ErrorCode, ErrorKind, Result};
use crate::data_provider::{DataProvider, col_index, is_column_only, parse_cell_ref};

/// Filesystem-based data provider that reads CSV and Excel files.
///
/// File paths are resolved relative to the configured base directory,
/// which is typically the directory containing the MDL model file.
pub struct FilesystemDataProvider {
    base_dir: PathBuf,
}

impl FilesystemDataProvider {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    pub(crate) fn resolve_path(&self, file: &str) -> Result<PathBuf> {
        let path = self.base_dir.join(file);
        // Normalize the path to resolve ../ components, then verify the
        // result stays within base_dir. This prevents model files from
        // referencing arbitrary files on the filesystem via absolute paths
        // or directory traversal.
        let canonical_base = self.base_dir.canonicalize().map_err(|e| {
            Error::new(
                ErrorKind::Import,
                ErrorCode::Generic,
                Some(format!("cannot canonicalize base directory: {e}")),
            )
        })?;
        let canonical_path = path.canonicalize().map_err(|e| {
            Error::new(
                ErrorKind::Import,
                ErrorCode::Generic,
                Some(format!("cannot resolve data file '{}': {e}", file)),
            )
        })?;
        if !canonical_path.starts_with(&canonical_base) {
            return Err(Error::new(
                ErrorKind::Import,
                ErrorCode::Generic,
                Some(format!("data file '{}' escapes base directory", file)),
            ));
        }
        Ok(canonical_path)
    }

    fn is_excel_file(path: &std::path::Path) -> bool {
        matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("xls" | "xlsx" | "xlsm")
        )
    }

    fn read_csv_records(&self, file: &str, delimiter: u8) -> Result<Vec<Vec<String>>> {
        let path = self.resolve_path(file)?;
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(false)
            .delimiter(delimiter)
            .flexible(true)
            .from_path(&path)
            .map_err(|e| {
                Error::new(
                    ErrorKind::Import,
                    ErrorCode::Generic,
                    Some(format!("failed to open '{}': {}", path.display(), e)),
                )
            })?;

        let mut records = Vec::new();
        for result in rdr.records() {
            let record = result.map_err(|e| {
                Error::new(
                    ErrorKind::Import,
                    ErrorCode::Generic,
                    Some(format!("CSV parse error in '{}': {}", file, e)),
                )
            })?;
            // Trim BOM and whitespace from fields
            records.push(
                record
                    .iter()
                    .map(|f| f.trim_start_matches('\u{feff}').to_string())
                    .collect(),
            );
        }
        Ok(records)
    }

    fn parse_delimiter(tab_or_delimiter: &str) -> u8 {
        match tab_or_delimiter {
            "," => b',',
            "\\t" | "\t" => b'\t',
            s if s.len() == 1 => s.as_bytes()[0],
            _ => b',',
        }
    }

    fn load_data_csv(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        time_col_or_row: &str,
        cell_label: &str,
    ) -> Result<Vec<(f64, f64)>> {
        let delimiter = Self::parse_delimiter(tab_or_delimiter);
        let records = self.read_csv_records(file, delimiter)?;

        if records.is_empty() {
            return Ok(Vec::new());
        }

        // Vensim GET DIRECT DATA has two addressing modes:
        // 1. time_col_or_row is a column letter (e.g. "A") - column-oriented
        // 2. time_col_or_row is a row number (e.g. "1") - row-oriented
        let time_col_or_row = time_col_or_row.trim();

        if !time_col_or_row.is_empty() && time_col_or_row.chars().all(|c| c.is_ascii_digit()) {
            // Row-oriented: time values are in the specified row
            self.load_data_csv_row_oriented(&records, file, time_col_or_row, cell_label)
        } else {
            // Column-oriented: time values are in the specified column
            self.load_data_csv_col_oriented(&records, file, time_col_or_row, cell_label)
        }
    }

    fn load_data_csv_col_oriented(
        &self,
        records: &[Vec<String>],
        file: &str,
        time_col: &str,
        cell_label: &str,
    ) -> Result<Vec<(f64, f64)>> {
        let time_col_idx = col_index(time_col)?;
        let (data_start_row, data_col_idx) = parse_cell_ref(cell_label)?;

        let mut pairs = Vec::new();
        for (row_idx, row) in records.iter().enumerate().skip(data_start_row) {
            let time_str = row.get(time_col_idx).map(|s| s.trim()).unwrap_or("");
            let val_str = row.get(data_col_idx).map(|s| s.trim()).unwrap_or("");

            if time_str.is_empty() || val_str.is_empty() {
                continue;
            }

            let time: f64 = time_str.parse().map_err(|_| {
                Error::new(
                    ErrorKind::Import,
                    ErrorCode::Generic,
                    Some(format!(
                        "non-numeric time value '{}' in '{}' at row {}",
                        time_str,
                        file,
                        row_idx + 1
                    )),
                )
            })?;
            let value: f64 = val_str.parse().map_err(|_| {
                Error::new(
                    ErrorKind::Import,
                    ErrorCode::Generic,
                    Some(format!(
                        "non-numeric data value '{}' in '{}' at row {}",
                        val_str,
                        file,
                        row_idx + 1
                    )),
                )
            })?;
            pairs.push((time, value));
        }
        Ok(pairs)
    }

    fn load_data_csv_row_oriented(
        &self,
        records: &[Vec<String>],
        file: &str,
        time_row: &str,
        cell_label: &str,
    ) -> Result<Vec<(f64, f64)>> {
        let time_row_num: usize = time_row.parse::<usize>().map_err(|_| {
            Error::new(
                ErrorKind::Import,
                ErrorCode::Generic,
                Some(format!("bad row number '{}' in '{}'", time_row, file)),
            )
        })?;
        if time_row_num == 0 {
            return Err(Error::new(
                ErrorKind::Import,
                ErrorCode::Generic,
                Some(format!(
                    "time row '{}' must be >= 1 (1-indexed) in '{}'",
                    time_row, file
                )),
            ));
        }
        let time_row_idx = time_row_num - 1;

        let (data_row_idx, data_start_col) = parse_cell_ref(cell_label)?;

        let time_row_data = records.get(time_row_idx).ok_or_else(|| {
            Error::new(
                ErrorKind::Import,
                ErrorCode::Generic,
                Some(format!("time row {} out of range in '{}'", time_row, file)),
            )
        })?;

        let data_row_data = records.get(data_row_idx).ok_or_else(|| {
            Error::new(
                ErrorKind::Import,
                ErrorCode::Generic,
                Some(format!(
                    "data row {} out of range in '{}'",
                    data_row_idx + 1,
                    file
                )),
            )
        })?;

        let mut pairs = Vec::new();
        let max_col = time_row_data.len().max(data_row_data.len());
        for col_idx in data_start_col..max_col {
            let time_str = time_row_data.get(col_idx).map(|s| s.trim()).unwrap_or("");
            let val_str = data_row_data.get(col_idx).map(|s| s.trim()).unwrap_or("");

            if time_str.is_empty() || val_str.is_empty() {
                continue;
            }

            let time: f64 = time_str.parse().map_err(|_| {
                Error::new(
                    ErrorKind::Import,
                    ErrorCode::Generic,
                    Some(format!(
                        "non-numeric time value '{}' in '{}' col {}",
                        time_str,
                        file,
                        col_idx + 1
                    )),
                )
            })?;
            let value: f64 = val_str.parse().map_err(|_| {
                Error::new(
                    ErrorKind::Import,
                    ErrorCode::Generic,
                    Some(format!(
                        "non-numeric data value '{}' in '{}' col {}",
                        val_str,
                        file,
                        col_idx + 1
                    )),
                )
            })?;
            pairs.push((time, value));
        }
        Ok(pairs)
    }

    fn load_constant_csv(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        row_label: &str,
        col_label: &str,
    ) -> Result<f64> {
        let delimiter = Self::parse_delimiter(tab_or_delimiter);
        let records = self.read_csv_records(file, delimiter)?;

        let (row_idx, col_idx) = parse_cell_ref(row_label)?;

        // For scalar constants, col_label is empty and the column comes
        // from row_label's cell reference (e.g. "B2" -> col B).
        // For arrayed constants, col_label overrides the column.
        let col_idx = if col_label.is_empty() {
            col_idx
        } else if is_column_only(col_label) {
            col_index(col_label)?
        } else {
            let (_r, c) = parse_cell_ref(col_label)?;
            c
        };

        let row = records.get(row_idx).ok_or_else(|| {
            Error::new(
                ErrorKind::Import,
                ErrorCode::Generic,
                Some(format!("row {} out of range in '{}'", row_idx + 1, file)),
            )
        })?;

        let val_str = row.get(col_idx).map(|s| s.trim()).unwrap_or("");
        val_str.parse().map_err(|_| {
            Error::new(
                ErrorKind::Import,
                ErrorCode::Generic,
                Some(format!(
                    "non-numeric value '{}' at ({},{}) in '{}'",
                    val_str,
                    row_idx + 1,
                    col_idx + 1,
                    file
                )),
            )
        })
    }

    fn load_subscript_csv(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        first_cell: &str,
        last_cell: &str,
    ) -> Result<Vec<String>> {
        let delimiter = Self::parse_delimiter(tab_or_delimiter);
        let records = self.read_csv_records(file, delimiter)?;

        let (start_row, start_col) = parse_cell_ref(first_cell)?;

        // last_cell can be:
        // - a full cell ref ("A5") - read to that cell
        // - just a column letter ("A") - read down to end of data
        // - just a row number ("2") - read across that row to end of data
        // - empty ("") - read down the start column to end of data
        let is_row_number =
            last_cell.trim().chars().all(|c| c.is_ascii_digit()) && !last_cell.trim().is_empty();
        let (end_row, end_col) = if is_row_number {
            let row_num: usize = last_cell.trim().parse::<usize>().map_err(|_| {
                Error::new(
                    ErrorKind::Import,
                    ErrorCode::Generic,
                    Some(format!(
                        "invalid row number '{}' in last_cell",
                        last_cell.trim()
                    )),
                )
            })?;
            if row_num == 0 {
                return Err(Error::new(
                    ErrorKind::Import,
                    ErrorCode::Generic,
                    Some(format!(
                        "row number '{}' in last_cell must be >= 1 (1-indexed)",
                        last_cell.trim()
                    )),
                ));
            }
            let row_idx = row_num - 1;
            let empty = Vec::new();
            let row = records.get(row_idx).unwrap_or(&empty);
            let last_col = if row.is_empty() {
                start_col
            } else {
                row.len() - 1
            };
            (row_idx, last_col)
        } else if is_column_only(last_cell.trim()) {
            let col = col_index(last_cell.trim())?;
            let mut last_row = start_row;
            for (i, row) in records.iter().enumerate().skip(start_row) {
                if let Some(val) = row.get(col)
                    && !val.trim().is_empty()
                {
                    last_row = i;
                }
            }
            (last_row, col)
        } else if last_cell.trim().is_empty() {
            let mut last_row = start_row;
            for (i, row) in records.iter().enumerate().skip(start_row) {
                if let Some(val) = row.get(start_col)
                    && !val.trim().is_empty()
                {
                    last_row = i;
                }
            }
            (last_row, start_col)
        } else {
            parse_cell_ref(last_cell)?
        };

        let mut elements = Vec::new();
        if start_col == end_col {
            for row_idx in start_row..=end_row {
                if let Some(row) = records.get(row_idx)
                    && let Some(val) = row.get(start_col)
                {
                    let trimmed = val.trim();
                    if !trimmed.is_empty() {
                        elements.push(trimmed.to_string());
                    }
                }
            }
        } else if let Some(row) = records.get(start_row) {
            for col_idx in start_col..=end_col {
                if let Some(val) = row.get(col_idx) {
                    let trimmed = val.trim();
                    if !trimmed.is_empty() {
                        elements.push(trimmed.to_string());
                    }
                }
            }
        }

        Ok(elements)
    }
}

impl DataProvider for FilesystemDataProvider {
    fn load_data(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        time_col_or_row: &str,
        cell_label: &str,
    ) -> Result<Vec<(f64, f64)>> {
        let path = self.resolve_path(file)?;
        if Self::is_excel_file(&path) {
            #[cfg(feature = "ext_data")]
            {
                return self.load_data_excel(file, tab_or_delimiter, time_col_or_row, cell_label);
            }
            #[cfg(not(feature = "ext_data"))]
            {
                return Err(Error::new(
                    ErrorKind::Import,
                    ErrorCode::Generic,
                    Some(format!(
                        "Excel file '{}' requires the 'ext_data' feature",
                        file
                    )),
                ));
            }
        }
        self.load_data_csv(file, tab_or_delimiter, time_col_or_row, cell_label)
    }

    fn load_constant(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        row_label: &str,
        col_label: &str,
    ) -> Result<f64> {
        let path = self.resolve_path(file)?;
        if Self::is_excel_file(&path) {
            #[cfg(feature = "ext_data")]
            {
                return self.load_constant_excel(file, tab_or_delimiter, row_label, col_label);
            }
            #[cfg(not(feature = "ext_data"))]
            {
                return Err(Error::new(
                    ErrorKind::Import,
                    ErrorCode::Generic,
                    Some(format!(
                        "Excel file '{}' requires the 'ext_data' feature",
                        file
                    )),
                ));
            }
        }
        self.load_constant_csv(file, tab_or_delimiter, row_label, col_label)
    }

    fn load_lookup(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        row_label: &str,
        col_label: &str,
    ) -> Result<Vec<(f64, f64)>> {
        let path = self.resolve_path(file)?;
        if Self::is_excel_file(&path) {
            #[cfg(feature = "ext_data")]
            {
                return self.load_data_excel(file, tab_or_delimiter, row_label, col_label);
            }
            #[cfg(not(feature = "ext_data"))]
            {
                return Err(Error::new(
                    ErrorKind::Import,
                    ErrorCode::Generic,
                    Some(format!(
                        "Excel file '{}' requires the 'ext_data' feature",
                        file
                    )),
                ));
            }
        }
        // Lookup uses the same format as data
        self.load_data_csv(file, tab_or_delimiter, row_label, col_label)
    }

    fn load_subscript(
        &self,
        file: &str,
        tab_or_delimiter: &str,
        first_cell: &str,
        last_cell: &str,
    ) -> Result<Vec<String>> {
        let path = self.resolve_path(file)?;
        if Self::is_excel_file(&path) {
            #[cfg(feature = "ext_data")]
            {
                return self.load_subscript_excel(file, tab_or_delimiter, first_cell, last_cell);
            }
            #[cfg(not(feature = "ext_data"))]
            {
                return Err(Error::new(
                    ErrorKind::Import,
                    ErrorCode::Generic,
                    Some(format!(
                        "Excel file '{}' requires the 'ext_data' feature",
                        file
                    )),
                ));
            }
        }
        self.load_subscript_csv(file, tab_or_delimiter, first_cell, last_cell)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn create_temp_csv(name: &str, content: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join(name);
        let mut f = std::fs::File::create(&file_path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        (dir, name.to_string())
    }

    #[test]
    fn test_load_data_column_oriented() {
        let (dir, file) =
            create_temp_csv("test.csv", "Year,Value\n2000,10.0\n2010,20.0\n2020,30.0\n");
        let provider = FilesystemDataProvider::new(dir.path());
        let result = provider.load_data(&file, ",", "A", "B2").unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], (2000.0, 10.0));
        assert_eq!(result[1], (2010.0, 20.0));
        assert_eq!(result[2], (2020.0, 30.0));
    }

    #[test]
    fn test_load_data_row_oriented() {
        let (dir, file) = create_temp_csv("test.csv", "m,1990,2005,2015\nM1,11,12,13\n");
        let provider = FilesystemDataProvider::new(dir.path());
        let result = provider.load_data(&file, ",", "1", "B2").unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], (1990.0, 11.0));
        assert_eq!(result[1], (2005.0, 12.0));
        assert_eq!(result[2], (2015.0, 13.0));
    }

    #[test]
    fn test_load_constant() {
        let (dir, file) = create_temp_csv("const.csv", "a,\n,2050\n");
        let provider = FilesystemDataProvider::new(dir.path());
        let result = provider.load_constant(&file, ",", "B2", "B").unwrap();
        assert_eq!(result, 2050.0);
    }

    #[test]
    fn test_load_subscript_vertical() {
        let (dir, file) = create_temp_csv("subs.csv", "DimB\nB1\nB2\nB3\n");
        let provider = FilesystemDataProvider::new(dir.path());
        let result = provider.load_subscript(&file, ",", "A2", "A").unwrap();
        assert_eq!(result, vec!["B1", "B2", "B3"]);
    }

    #[test]
    fn test_load_subscript_horizontal() {
        let (dir, file) = create_temp_csv("subs.csv", "DimC,,\nC1,C2,C3\n");
        let provider = FilesystemDataProvider::new(dir.path());
        let result = provider.load_subscript(&file, ",", "A2", "C2").unwrap();
        assert_eq!(result, vec!["C1", "C2", "C3"]);
    }

    #[test]
    fn test_tab_delimiter() {
        let (dir, file) = create_temp_csv("test.tsv", "Year\tValue\n2000\t10.0\n2010\t20.0\n");
        let provider = FilesystemDataProvider::new(dir.path());
        let result = provider.load_data(&file, "\\t", "A", "B2").unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], (2000.0, 10.0));
    }

    #[test]
    fn test_load_data_with_real_e_data_csv() {
        let test_dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/sdeverywhere/models/directdata"
        );
        let provider = FilesystemDataProvider::new(test_dir);
        // e_data.csv has BOM, columns: Year,A1,A2
        // Data starts at row 2 (B2 means column B = A1, starting from row 2)
        let result = provider.load_data("e_data.csv", ",", "A", "B2").unwrap();
        assert_eq!(result.len(), 5);
        assert_eq!(result[0], (1990.0, 610.0));
        assert_eq!(result[4], (2050.0, 583.0));
    }

    #[test]
    fn test_load_data_with_real_g_data_csv() {
        let test_dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/sdeverywhere/models/directdata"
        );
        let provider = FilesystemDataProvider::new(test_dir);
        let result = provider.load_data("g_data.csv", ",", "A", "B2").unwrap();
        assert_eq!(result.len(), 13);
        assert_eq!(result[0], (1990.0, 97.0));
    }

    #[test]
    fn test_load_subscript_with_real_b_subs_csv() {
        let test_dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/sdeverywhere/models/directsubs"
        );
        let provider = FilesystemDataProvider::new(test_dir);
        let result = provider
            .load_subscript("b_subs.csv", ",", "A2", "A")
            .unwrap();
        assert_eq!(result, vec!["B1", "B2", "B3"]);
    }

    #[test]
    fn test_load_subscript_with_real_c_subs_csv() {
        let test_dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/sdeverywhere/models/directsubs"
        );
        let provider = FilesystemDataProvider::new(test_dir);
        let result = provider
            .load_subscript("c_subs.csv", ",", "A2", "C2")
            .unwrap();
        assert_eq!(result, vec!["C1", "C2", "C3"]);
    }

    #[test]
    fn test_load_data_row_oriented_with_real_m_csv() {
        let test_dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/sdeverywhere/models/directdata"
        );
        let provider = FilesystemDataProvider::new(test_dir);
        // m.csv: row-oriented, time is row 1, data starts at B2
        let result = provider.load_data("m.csv", ",", "1", "B2").unwrap();
        assert_eq!(result.len(), 5);
        assert_eq!(result[0], (1990.0, 11.0));
        assert_eq!(result[4], (2050.0, 15.0));
    }

    #[test]
    fn test_load_data_col_oriented_with_real_mt_csv() {
        let test_dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/sdeverywhere/models/directdata"
        );
        let provider = FilesystemDataProvider::new(test_dir);
        // mt.csv: column-oriented, time in column A, data starts at B2
        let result = provider.load_data("mt.csv", ",", "A", "B2").unwrap();
        assert_eq!(result.len(), 5);
        assert_eq!(result[0], (1990.0, 11.0));
        assert_eq!(result[4], (2050.0, 15.0));
    }

    #[test]
    fn test_missing_file_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let provider = FilesystemDataProvider::new(dir.path());
        let result = provider.load_data("nonexistent.csv", ",", "A", "B2");
        assert!(result.is_err());
    }

    #[test]
    fn test_load_subscript_row_number_zero_returns_error() {
        let (dir, file) = create_temp_csv("subs.csv", "DimB\nB1\nB2\nB3\n");
        let provider = FilesystemDataProvider::new(dir.path());
        let result = provider.load_subscript(&file, ",", "A1", "0");
        assert!(
            result.is_err(),
            "row number '0' should return an error, not panic"
        );
    }

    #[test]
    fn test_load_subscript_row_number_non_numeric_returns_error() {
        let (dir, file) = create_temp_csv("subs.csv", "DimB\nB1\nB2\nB3\n");
        let provider = FilesystemDataProvider::new(dir.path());
        let result = provider.load_subscript(&file, ",", "A1", "999999999999999999999");
        assert!(
            result.is_err(),
            "overflow row number should return an error, not panic"
        );
    }

    #[test]
    fn test_resolve_path_rejects_directory_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let provider = FilesystemDataProvider::new(dir.path());
        let result = provider.load_data("../../../etc/passwd", ",", "A", "B2");
        assert!(result.is_err(), "path traversal via ../ should be rejected");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("outside") || err_msg.contains("escapes"),
            "error should mention path escaping the base directory, got: {err_msg}"
        );
    }

    #[test]
    fn test_resolve_path_rejects_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let provider = FilesystemDataProvider::new(dir.path());
        let result = provider.load_data("/etc/passwd", ",", "A", "B2");
        assert!(result.is_err(), "absolute paths should be rejected");
    }

    #[test]
    fn test_row_oriented_time_row_zero_returns_error() {
        let (dir, file) = create_temp_csv("test.csv", "m,1990,2005\nM1,11,12\n");
        let provider = FilesystemDataProvider::new(dir.path());
        let result = provider.load_data(&file, ",", "0", "B2");
        assert!(
            result.is_err(),
            "time_row '0' should return an error, not underflow"
        );
    }
}
