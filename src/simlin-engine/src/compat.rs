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
use crate::xmile;

pub fn to_xmile(project: &Project) -> Result<String> {
    xmile::project_to_xmile(project)
}

pub fn to_mdl(project: &Project) -> Result<String> {
    mdl::project_to_mdl(project)
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

#[cfg(feature = "file_io")]
pub fn load_dat(file_path: &str) -> StdResult<Results, Box<dyn Error>> {
    use float_cmp::approx_eq;

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
                .expect("dat file has no data");
            let it = longest.first().map(|p| p.0).unwrap_or(0.0);
            let ft = longest.last().map(|p| p.0).unwrap_or(1.0);
            let sp = if longest.len() >= 2 {
                longest[1].0 - longest[0].0
            } else {
                1.0
            };
            (it, ft, sp)
        };

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
                if data_time > t && !approx_eq!(f64, data_time, t) {
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
            start: 0.0,
            stop: 0.0,
            dt: 0.0,
            save_step: 0.0,
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
            let ident = Ident::<Canonical>::new(name);
            (
                Ident::<Canonical>::from_unchecked(ident.to_source_repr()),
                i,
            )
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
            row[i] = match f64::from_str(field.trim()) {
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
