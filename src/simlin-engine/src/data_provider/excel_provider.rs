// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use calamine::{Data, DataType, Reader, open_workbook_auto};

/// Convert a calamine `Data` cell to a string representation, handling
/// numeric cells that `as_string()` ignores.
fn data_to_string(val: &Data) -> Option<String> {
    match val {
        Data::String(s) => Some(s.clone()),
        Data::Float(f) => {
            if f.is_finite()
                && f.fract() == 0.0
                && *f >= (i128::MIN as f64)
                && *f < (i128::MAX as f64)
            {
                Some(format!("{}", *f as i128))
            } else {
                Some(format!("{f}"))
            }
        }
        Data::Int(i) => Some(format!("{i}")),
        Data::Bool(b) => Some(format!("{b}")),
        Data::Empty
        | Data::Error(_)
        | Data::DateTime(_)
        | Data::DateTimeIso(_)
        | Data::DurationIso(_) => None,
    }
}

use crate::common::{Error, ErrorCode, ErrorKind, Result};
use crate::data_provider::{col_index, is_column_only, parse_cell_ref, parse_row_or_cell_ref};

use super::csv_provider::FilesystemDataProvider;

impl FilesystemDataProvider {
    pub(crate) fn load_data_excel(
        &self,
        file: &str,
        sheet_name: &str,
        time_col_or_row: &str,
        cell_label: &str,
    ) -> Result<Vec<(f64, f64)>> {
        let range = self.open_sheet(file, sheet_name)?;
        let time_col_or_row = time_col_or_row.trim();

        if !time_col_or_row.is_empty() && time_col_or_row.chars().all(|c| c.is_ascii_digit()) {
            self.load_data_excel_row_oriented(&range, file, time_col_or_row, cell_label)
        } else {
            self.load_data_excel_col_oriented(&range, file, time_col_or_row, cell_label)
        }
    }

    fn load_data_excel_col_oriented(
        &self,
        range: &calamine::Range<Data>,
        file: &str,
        time_col: &str,
        cell_label: &str,
    ) -> Result<Vec<(f64, f64)>> {
        let time_col_idx = col_index(time_col)? as u32;
        let (data_start_row, data_col_idx) = parse_cell_ref(cell_label)?;
        let data_start_row = data_start_row as u32;
        let data_col_idx = data_col_idx as u32;

        let (height, _width) = range.get_size();
        let start = range.start().unwrap_or((0, 0));

        let mut pairs = Vec::new();
        for row in data_start_row..(start.0 + height as u32) {
            let time_val = range.get_value((row, time_col_idx));
            let data_val = range.get_value((row, data_col_idx));

            let (time, value) = match (time_val, data_val) {
                (Some(t), Some(d)) => {
                    if matches!(t, Data::Empty) || matches!(d, Data::Empty) {
                        continue;
                    }
                    let t = t.as_f64().ok_or_else(|| {
                        Error::new(
                            ErrorKind::Import,
                            ErrorCode::Generic,
                            Some(format!(
                                "non-numeric time value at row {} in '{}'",
                                row + 1,
                                file
                            )),
                        )
                    })?;
                    let d = d.as_f64().ok_or_else(|| {
                        Error::new(
                            ErrorKind::Import,
                            ErrorCode::Generic,
                            Some(format!(
                                "non-numeric data value at row {} in '{}'",
                                row + 1,
                                file
                            )),
                        )
                    })?;
                    (t, d)
                }
                _ => continue,
            };
            pairs.push((time, value));
        }
        Ok(pairs)
    }

    fn load_data_excel_row_oriented(
        &self,
        range: &calamine::Range<Data>,
        file: &str,
        time_row: &str,
        cell_label: &str,
    ) -> Result<Vec<(f64, f64)>> {
        let time_row_num: u32 = time_row.parse::<u32>().map_err(|_| {
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
        let data_row_idx = data_row_idx as u32;
        let data_start_col = data_start_col as u32;

        let (_height, width) = range.get_size();
        let start = range.start().unwrap_or((0, 0));

        let mut pairs = Vec::new();
        for col in data_start_col..(start.1 + width as u32) {
            let time_val = range.get_value((time_row_idx, col));
            let data_val = range.get_value((data_row_idx, col));

            let (time, value) = match (time_val, data_val) {
                (Some(t), Some(d)) => {
                    if matches!(t, Data::Empty) || matches!(d, Data::Empty) {
                        continue;
                    }
                    let t = t.as_f64().ok_or_else(|| {
                        Error::new(
                            ErrorKind::Import,
                            ErrorCode::Generic,
                            Some(format!(
                                "non-numeric time value at col {} in '{}'",
                                col + 1,
                                file
                            )),
                        )
                    })?;
                    let d = d.as_f64().ok_or_else(|| {
                        Error::new(
                            ErrorKind::Import,
                            ErrorCode::Generic,
                            Some(format!(
                                "non-numeric data value at col {} in '{}'",
                                col + 1,
                                file
                            )),
                        )
                    })?;
                    (t, d)
                }
                _ => continue,
            };
            pairs.push((time, value));
        }
        Ok(pairs)
    }

    pub(crate) fn load_constant_excel(
        &self,
        file: &str,
        sheet_name: &str,
        row_label: &str,
        col_label: &str,
    ) -> Result<f64> {
        let range = self.open_sheet(file, sheet_name)?;

        let (row_idx, col_idx) = parse_row_or_cell_ref(row_label)?;
        // When col_label is present, it overrides the column from row_label.
        // This handles 4-argument GET DIRECT CONSTANTS calls where row and
        // column are specified separately.
        let col_idx = if col_label.is_empty() {
            col_idx
        } else if is_column_only(col_label) {
            col_index(col_label)?
        } else {
            let (_r, c) = parse_cell_ref(col_label)?;
            c
        };

        let val = range
            .get_value((row_idx as u32, col_idx as u32))
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::Import,
                    ErrorCode::Generic,
                    Some(format!(
                        "cell ({},{}) out of range in '{}'",
                        row_idx + 1,
                        col_idx + 1,
                        file
                    )),
                )
            })?;

        val.as_f64().ok_or_else(|| {
            Error::new(
                ErrorKind::Import,
                ErrorCode::Generic,
                Some(format!(
                    "non-numeric value at ({},{}) in '{}'",
                    row_idx + 1,
                    col_idx + 1,
                    file
                )),
            )
        })
    }

    pub(crate) fn load_subscript_excel(
        &self,
        file: &str,
        sheet_name: &str,
        first_cell: &str,
        last_cell: &str,
    ) -> Result<Vec<String>> {
        let range = self.open_sheet(file, sheet_name)?;

        let (start_row, start_col) = parse_cell_ref(first_cell)?;
        let start_row = start_row as u32;
        let start_col = start_col as u32;

        let is_row_number =
            last_cell.trim().chars().all(|c| c.is_ascii_digit()) && !last_cell.trim().is_empty();
        let (end_row, end_col) = if is_row_number {
            let row_num: u32 = last_cell.trim().parse::<u32>().map_err(|_| {
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
            let (height, width) = range.get_size();
            let range_start = range.start().unwrap_or((0, 0));
            let max_row = if height == 0 {
                range_start.0
            } else {
                range_start.0 + height as u32 - 1
            };
            let row_idx = (row_num - 1).min(max_row);
            let mut last_col = start_col;
            for col in start_col..(range_start.1 + width as u32) {
                if let Some(val) = range.get_value((row_idx, col))
                    && data_to_string(val).is_some_and(|s| !s.trim().is_empty())
                {
                    last_col = col;
                }
            }
            (row_idx, last_col)
        } else if is_column_only(last_cell.trim()) {
            let col = col_index(last_cell.trim())? as u32;
            let (height, _) = range.get_size();
            let range_start = range.start().unwrap_or((0, 0));
            let mut last_row = start_row;
            for row in start_row..(range_start.0 + height as u32) {
                if let Some(val) = range.get_value((row, col))
                    && data_to_string(val).is_some_and(|s| !s.trim().is_empty())
                {
                    last_row = row;
                }
            }
            (last_row, col)
        } else if last_cell.trim().is_empty() {
            let (height, _) = range.get_size();
            let range_start = range.start().unwrap_or((0, 0));
            let mut last_row = start_row;
            for row in start_row..(range_start.0 + height as u32) {
                if let Some(val) = range.get_value((row, start_col))
                    && data_to_string(val).is_some_and(|s| !s.trim().is_empty())
                {
                    last_row = row;
                }
            }
            (last_row, start_col)
        } else {
            let (r, c) = parse_cell_ref(last_cell)?;
            (r as u32, c as u32)
        };

        let mut elements = Vec::new();
        if start_col == end_col {
            for row in start_row..=end_row {
                if let Some(val) = range.get_value((row, start_col))
                    && let Some(s) = data_to_string(val)
                {
                    let trimmed = s.trim().to_string();
                    if !trimmed.is_empty() {
                        elements.push(trimmed);
                    }
                }
            }
        } else {
            for col in start_col..=end_col {
                if let Some(val) = range.get_value((start_row, col))
                    && let Some(s) = data_to_string(val)
                {
                    let trimmed = s.trim().to_string();
                    if !trimmed.is_empty() {
                        elements.push(trimmed);
                    }
                }
            }
        }

        Ok(elements)
    }

    fn open_sheet(&self, file: &str, sheet_name: &str) -> Result<calamine::Range<Data>> {
        let path = self.resolve_path(file)?;
        let mut workbook = open_workbook_auto(&path).map_err(|e| {
            Error::new(
                ErrorKind::Import,
                ErrorCode::Generic,
                Some(format!("failed to open Excel file '{}': {}", file, e)),
            )
        })?;

        workbook.worksheet_range(sheet_name).map_err(|e| {
            Error::new(
                ErrorKind::Import,
                ErrorCode::Generic,
                Some(format!(
                    "sheet '{}' not found in '{}': {}",
                    sheet_name, file, e
                )),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::csv_provider::FilesystemDataProvider;
    use crate::data_provider::DataProvider;

    fn test_data_dir() -> &'static str {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/sdeverywhere/models/directdata"
        )
    }

    #[test]
    fn test_load_data_excel_col_oriented() {
        let provider = FilesystemDataProvider::new(test_data_dir());
        // data.xlsx "A Data" sheet has time in col A, data starting at B2
        let result = provider
            .load_data("data.xlsx", "A Data", "A", "B2")
            .unwrap();
        assert!(!result.is_empty());
        // First time value should be 1990
        assert_eq!(result[0].0, 1990.0);
    }

    #[test]
    fn test_load_data_excel_dispatches_by_extension() {
        let provider = FilesystemDataProvider::new(test_data_dir());
        // Calling load_data on a .xlsx file should use Excel path
        let result = provider.load_data("data.xlsx", "A Data", "A", "B2");
        assert!(result.is_ok());
    }

    #[test]
    fn test_load_data_excel_missing_sheet() {
        let provider = FilesystemDataProvider::new(test_data_dir());
        let result = provider.load_data("data.xlsx", "Nonexistent Sheet", "A", "B2");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.details.as_ref().unwrap().contains("Nonexistent Sheet"));
    }

    #[test]
    fn test_load_subscript_excel_row_number_endpoint() {
        let provider = FilesystemDataProvider::new(test_data_dir());
        let result = provider.load_subscript("data.xlsx", "A Data", "A2", "2");
        assert!(
            result.is_ok(),
            "row-oriented last_cell should be supported for Excel files: {result:?}"
        );
    }

    #[test]
    fn data_to_string_handles_numeric_cells() {
        use super::data_to_string;
        use calamine::Data;

        assert_eq!(data_to_string(&Data::Float(5.0)), Some("5".to_string()));
        assert_eq!(data_to_string(&Data::Float(2.75)), Some("2.75".to_string()));
        assert_eq!(data_to_string(&Data::Int(42)), Some("42".to_string()));
        assert_eq!(
            data_to_string(&Data::String("hello".to_string())),
            Some("hello".to_string())
        );
        assert_eq!(data_to_string(&Data::Bool(true)), Some("true".to_string()));
        assert_eq!(data_to_string(&Data::Empty), None);
    }

    #[test]
    fn data_to_string_integer_floats_have_no_decimal() {
        use super::data_to_string;
        use calamine::Data;

        assert_eq!(data_to_string(&Data::Float(0.0)), Some("0".to_string()));
        assert_eq!(data_to_string(&Data::Float(10.0)), Some("10".to_string()));
        assert_eq!(data_to_string(&Data::Float(-5.0)), Some("-5".to_string()));
    }

    #[test]
    fn data_to_string_large_integer_floats() {
        use super::data_to_string;
        use calamine::Data;

        // f64 near i64::MAX where `as i64` saturates to a wrong value:
        // 9223372036854776000.0 as i64 = i64::MAX = 9223372036854775807
        // but the roundtrip (i64::MAX as f64) happens to equal the original,
        // so the old code would format as "9223372036854775807" (wrong).
        let large = 9_223_372_036_854_776_000.0_f64;
        let result = data_to_string(&Data::Float(large)).unwrap();
        assert!(
            !result.contains("9223372036854775807"),
            "large float must not be truncated to i64::MAX: {result}"
        );
    }

    #[test]
    fn data_to_string_beyond_i128_range() {
        use super::data_to_string;
        use calamine::Data;

        // f64 values beyond i128::MAX (~1.7e38) that are integer-valued
        // must not saturate to i128::MAX
        let huge = 1.0e39_f64;
        assert!(huge.is_finite() && huge.fract() == 0.0);
        let result = data_to_string(&Data::Float(huge)).unwrap();
        let i128_max_str = format!("{}", i128::MAX);
        assert!(
            !result.contains(&i128_max_str),
            "value beyond i128 range must not saturate to i128::MAX: {result}"
        );
        // Should fall back to float formatting
        assert!(
            result.contains("e+") || result.contains("E+") || result.parse::<f64>().is_ok(),
            "should produce a valid numeric string: {result}"
        );

        // Negative beyond i128::MIN
        let huge_neg = -1.0e39_f64;
        assert!(huge_neg.is_finite() && huge_neg.fract() == 0.0);
        let result_neg = data_to_string(&Data::Float(huge_neg)).unwrap();
        let i128_min_str = format!("{}", i128::MIN);
        assert!(
            !result_neg.contains(&i128_min_str),
            "value beyond i128 range must not saturate to i128::MIN: {result_neg}"
        );
    }
}
