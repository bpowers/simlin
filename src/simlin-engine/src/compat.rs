// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::io::BufRead;
#[cfg(any(feature = "file_io", feature = "xmutil"))]
use std::io::BufReader;

use crate::common::Result;
use crate::datamodel::Project;

#[cfg(feature = "file_io")]
use std::collections::HashMap;
#[cfg(feature = "file_io")]
use std::error::Error;
#[cfg(feature = "file_io")]
use std::fs::File;
#[cfg(feature = "file_io")]
use std::result::Result as StdResult;

#[cfg(feature = "file_io")]
use crate::common::{Canonical, Ident};
#[cfg(feature = "file_io")]
use crate::results::Method;
#[cfg(feature = "file_io")]
use crate::results::{Results, Specs};

use crate::mdl;
use crate::systems;
use crate::xmile;

pub fn to_xmile(project: &Project) -> Result<String> {
    xmile::project_to_xmile(project)
}

pub fn to_mdl(project: &Project) -> Result<String> {
    mdl::project_to_mdl(project)
}

/// Convert to Vensim MDL text, also returning any [`mdl::ExportWarning`]s for
/// constructs that could not be represented losslessly (#856).
pub fn to_mdl_with_warnings(project: &Project) -> Result<(String, Vec<mdl::ExportWarning>)> {
    mdl::project_to_mdl_with_warnings(project)
}

pub fn to_systems(project: &Project) -> Result<String> {
    systems::project_to_systems(project)
}

#[cfg(feature = "xmutil")]
pub fn open_vensim_xmutil(contents: &str) -> Result<Project> {
    use crate::common::{Error, ErrorCode, ErrorKind};
    use xmutil::convert_vensim_mdl;

    let (xmile_src, logs) = convert_vensim_mdl(contents, false);
    if xmile_src.is_none() {
        return Err(Error::new(
            ErrorKind::Import,
            ErrorCode::VensimConversion,
            Some("xmutil error: ".to_owned() + logs.as_ref().unwrap_or(&"(no logs)".to_owned())),
        ));
    }
    let xmile_src = xmile_src.unwrap();
    let mut f = BufReader::new(xmile_src.as_bytes());
    xmile::project_from_reader(&mut f)
}

/// Parse a Vensim MDL file using the native Rust parser.
pub fn open_vensim(contents: &str) -> Result<Project> {
    open_vensim_with_data(contents, None)
}

/// Parse a Vensim MDL file with an optional DataProvider for resolving
/// GET DIRECT external data references (CSV, Excel).
pub fn open_vensim_with_data(
    contents: &str,
    data_provider: Option<&dyn crate::data_provider::DataProvider>,
) -> Result<Project> {
    mdl::parse_mdl_with_data(contents, data_provider)
}

pub fn open_xmile(reader: &mut dyn BufRead) -> Result<Project> {
    xmile::project_from_reader(reader)
}

/// Parse a systems format file and translate it to a Project.
///
/// Uses a default simulation duration of 10 rounds. Callers that
/// need a different duration should use `systems::parse` and
/// `systems::translate::translate` directly.
pub fn open_systems(contents: &str) -> Result<Project> {
    let model = systems::parse(contents)?;
    systems::translate::translate(&model, systems::translate::DEFAULT_ROUNDS)
}

#[cfg(feature = "file_io")]
pub fn load_dat(file_path: &str) -> StdResult<Results, Box<dyn Error>> {
    use crate::float::approx_eq;

    let file = File::open(file_path)?;

    let unprocessed = {
        let mut unprocessed: HashMap<String, Vec<(f64, f64)>> = HashMap::new();

        let mut curr: Vec<(f64, f64)> = vec![];
        let mut ident: Option<String> = None;

        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.contains('\t') {
                use std::str::FromStr;
                let parts = line.split('\t').collect::<Vec<_>>();
                let l = parts[0].trim();
                let r = parts[1].trim();
                curr.push((f64::from_str(l)?, f64::from_str(r)?));
            } else {
                if let Some(id) = ident.take() {
                    assert!(unprocessed.insert(id, std::mem::take(&mut curr)).is_none());
                }
                let name = Ident::<Canonical>::new(line.trim());
                ident = Some(name.to_source_repr());
            }
        }
        if let Some(id) = ident.take() {
            assert!(unprocessed.insert(id, std::mem::take(&mut curr)).is_none());
        }
        unprocessed
    };

    let offsets: HashMap<Ident<Canonical>, usize> = unprocessed
        .keys()
        .enumerate()
        .map(|(i, r)| (Ident::<Canonical>::from_str_unchecked(r.as_str()), i))
        .collect();

    // Infer simulation parameters from data when not explicitly present
    let (initial_time, final_time, saveper) =
        if unprocessed.contains_key("initial_time") && unprocessed.contains_key("final_time") {
            let it = unprocessed["initial_time"][0].1;
            let ft = unprocessed["final_time"][0].1;
            let sp = if unprocessed.contains_key("saveper") {
                unprocessed["saveper"][0].1
            } else {
                1.0
            };
            (it, ft, sp)
        } else {
            // Find the variable with the most data points to infer time range
            let longest = unprocessed
                .values()
                .max_by_key(|v| v.len())
                .ok_or("dat file has no data")?;
            let it = longest.first().map(|p| p.0).unwrap_or(0.0);
            let ft = longest.last().map(|p| p.0).unwrap_or(1.0);
            let sp = if longest.len() >= 2 {
                longest[1].0 - longest[0].0
            } else {
                1.0
            };
            (it, ft, sp)
        };

    if saveper <= 0.0 {
        return Err("inferred saveper is <= 0 (duplicate or unsorted timestamps)".into());
    }

    let step_size = unprocessed.len();
    let step_count = ((final_time - initial_time) / saveper).ceil() as usize + 1;
    let mut step_data: Vec<f64> = Vec::with_capacity(step_count * step_size);
    step_data.extend(std::iter::repeat_n(f64::NAN, step_count * step_size));

    for (ident, var_off) in offsets.iter() {
        let data = &unprocessed[ident.as_str()];
        let mut data_iter = data.iter().cloned().peekable();
        let mut last_value: f64 = f64::NAN;
        for step in 0..step_count {
            let t: f64 = initial_time + saveper * (step as f64);
            // Advance past data points at or before the current time,
            // keeping the most recent value (sample-and-hold).
            while let Some(&(data_time, value)) = data_iter.peek() {
                if data_time > t && !approx_eq(data_time, t) {
                    break;
                }
                last_value = value;
                data_iter.next();
            }
            step_data[step * step_size + var_off] = last_value;
        }
    }

    Ok(Results {
        offsets,
        data: step_data.into_boxed_slice(),
        step_size,
        step_count,
        specs: Specs {
            start: initial_time,
            stop: final_time,
            dt: saveper,
            save_step: saveper,
            method: Method::Euler,
            n_chunks: step_count,
        },
        is_vensim: true,
    })
}

#[cfg(feature = "file_io")]
pub fn load_csv(file_path: &str, delimiter: u8) -> StdResult<Results, Box<dyn Error>> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .from_path(file_path)?;

    let header = rdr.headers().unwrap();
    let offsets: HashMap<Ident<Canonical>, usize> = header
        .iter()
        .enumerate()
        .map(|(i, r)| {
            // stella outputs the first 'time' column as the time _units_, which is bonkers
            let name = if i == 0 { "time" } else { r };
            (Ident::new(name), i)
        })
        .collect();

    let step_size = offsets.len();
    let mut step_data: Vec<Vec<f64>> = Vec::new();
    let mut step_count = 0;

    for result in rdr.records() {
        let record = result?;

        let mut row = vec![0.0; step_size];
        for (i, field) in record.iter().enumerate() {
            use std::str::FromStr;
            let field = field.trim();
            if field.is_empty() {
                // Vensim's `.tab` / `.csv` export writes a constant's value on
                // the first data row and leaves the cell EMPTY on every row
                // after, so an empty cell means "unchanged from the previous
                // step" -- not 0, and not a parse error. Carrying the previous
                // value forward is the only reading that reproduces the run.
                // An empty cell on the FIRST row has nothing to carry and stays
                // a hard error.
                let Some(prev) = step_data.last() else {
                    return Err(format!(
                        "{file_path}: empty value for column {i} on the first data row"
                    )
                    .into());
                };
                row[i] = prev[i];
                continue;
            }
            row[i] = match f64::from_str(field) {
                Ok(n) => n,
                Err(err) => {
                    return Err(Box::new(err));
                }
            };
        }

        step_data.push(row);
        step_count += 1;
    }

    let step_data: Vec<f64> = step_data.into_iter().flatten().collect();

    Ok(Results {
        offsets,
        data: step_data.into_boxed_slice(),
        step_size,
        step_count,
        specs: Specs {
            start: 0.0,
            stop: 0.0,
            dt: 0.0,
            save_step: 0.0,
            method: Method::Euler,
            n_chunks: step_count,
        },
        is_vensim: false,
    })
}

#[cfg(all(test, feature = "file_io"))]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn load_dat_empty_file_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.dat");
        std::fs::File::create(&path).unwrap();

        let result = load_dat(path.to_str().unwrap());
        assert!(
            result.is_err(),
            "empty .dat file should return Err, not panic"
        );
    }

    #[test]
    fn load_dat_duplicate_timestamps_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dup.dat");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "some_var").unwrap();
        writeln!(f, "0\t1.0").unwrap();
        writeln!(f, "0\t2.0").unwrap();
        writeln!(f, "0\t3.0").unwrap();

        let result = load_dat(path.to_str().unwrap());
        assert!(result.is_err(), "duplicate timestamps should return Err");
    }

    #[test]
    fn load_dat_valid_file_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("valid.dat");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "test_var").unwrap();
        writeln!(f, "0\t10.0").unwrap();
        writeln!(f, "1\t20.0").unwrap();
        writeln!(f, "2\t30.0").unwrap();

        let result = load_dat(path.to_str().unwrap());
        assert!(
            result.is_ok(),
            "valid .dat file should succeed: {:?}",
            result.err()
        );
    }

    /// Write `contents` to a temp `.tab` and load it. Returns the temp dir so the
    /// caller keeps it alive for the duration of the assertions.
    fn load_tab(contents: &str) -> (tempfile::TempDir, StdResult<Results, Box<dyn Error>>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.tab");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        drop(f);
        let result = load_csv(path.to_str().unwrap(), b'\t');
        (dir, result)
    }

    fn column(results: &Results, name: &str) -> Vec<f64> {
        let off = results.offsets[&Ident::new(name)];
        (0..results.step_count)
            .map(|step| results.data[step * results.step_size + off])
            .collect()
    }

    /// Vensim writes a CONSTANT's value on the first data row and leaves the cell
    /// empty on every row after. An empty cell therefore means "unchanged", so it
    /// carries the previous step's value forward -- reading it as `0` (or as a
    /// parse error) misreports the run.
    #[test]
    fn load_csv_carries_an_empty_cell_forward() {
        let (_dir, result) = load_tab("Time\tvarying\tconstant\n0\t1.0\t7.5\n1\t2.0\t\n2\t3.0\t\n");
        let results = result.expect("elided constants must load");

        assert_eq!(vec![0.0, 1.0, 2.0], column(&results, "time"));
        assert_eq!(vec![1.0, 2.0, 3.0], column(&results, "varying"));
        assert_eq!(
            vec![7.5, 7.5, 7.5],
            column(&results, "constant"),
            "an empty cell means unchanged, not 0"
        );
    }

    /// Whitespace is not a value either: the cell is trimmed before the emptiness
    /// test, so a `.tab` padded with spaces carries forward the same way.
    #[test]
    fn load_csv_carries_a_whitespace_only_cell_forward() {
        let (_dir, result) = load_tab("Time\tconstant\n0\t7.5\n1\t   \n");
        let results = result.expect("whitespace-padded constants must load");
        assert_eq!(vec![7.5, 7.5], column(&results, "constant"));
    }

    /// The FIRST data row has nothing to carry forward, so an empty cell there is
    /// a genuinely missing value and must be loud rather than silently 0.
    #[test]
    fn load_csv_rejects_an_empty_cell_on_the_first_row() {
        let (_dir, result) = load_tab("Time\tconstant\n0\t\n1\t7.5\n");
        let err = result.expect_err("an empty first-row cell must be an error");
        let msg = err.to_string();
        assert!(
            msg.contains("first data row"),
            "the error should say which row, got: {msg}"
        );
        assert!(
            msg.contains("column 1"),
            "the error should say which column, got: {msg}"
        );
    }

    /// A row with a different field count than the header is a corrupt file. The
    /// csv reader's default `flexible(false)` catches it -- pinned here because the
    /// carry-forward branch indexes `prev[i]` and would panic on a longer row.
    #[test]
    fn load_csv_rejects_a_ragged_row() {
        let (_dir, result) = load_tab("Time\ta\tb\n0\t1.0\t2.0\n1\t3.0\n");
        assert!(
            result.is_err(),
            "a row with too few fields must be an error, not a silent short row"
        );
    }

    /// A non-numeric cell is still a hard error; the carry-forward branch only
    /// fires on an EMPTY cell.
    #[test]
    fn load_csv_rejects_a_non_numeric_cell() {
        let (_dir, result) = load_tab("Time\ta\n0\t1.0\n1\tnope\n");
        assert!(result.is_err(), "a non-numeric cell must be an error");
    }
}
