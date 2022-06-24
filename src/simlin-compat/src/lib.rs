// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::result::Result as StdResult;

use simlin_engine::datamodel::Project;
pub use simlin_engine::{self as engine, prost, Result, Results};
use simlin_engine::{canonicalize, quoteize, Method, SimSpecs};

pub mod xmile;

pub fn to_xmile(project: &Project) -> Result<String> {
    xmile::project_to_xmile(project)
}

#[cfg(feature = "vensim")]
pub fn open_vensim(reader: &mut dyn BufRead) -> Result<Project> {
    use simlin_engine::common::{Error, ErrorCode, ErrorKind};
    use xmutil::convert_vensim_mdl;

    let mut contents_buf: Vec<u8> = vec![];
    reader
        .read_until(0, &mut contents_buf)
        .map_err(|_err| Error::new(ErrorKind::Import, ErrorCode::VensimConversion, None))?;
    let contents: String = String::from_utf8(contents_buf).unwrap();
    let (xmile_src, _logs) = convert_vensim_mdl(&contents, false);
    if xmile_src.is_none() {
        return Err(Error::new(
            ErrorKind::Import,
            ErrorCode::VensimConversion,
            Some("unknown xmutil error".to_owned()),
        ));
    }
    let xmile_src = xmile_src.unwrap();
    let mut f = BufReader::new(xmile_src.as_bytes());
    xmile::project_from_reader(&mut f)
}

pub fn open_xmile(reader: &mut dyn BufRead) -> Result<Project> {
    xmile::project_from_reader(reader)
}

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
                let name = canonicalize(line.trim());
                ident = Some(quoteize(&name));
            }
        }
        if let Some(id) = ident.take() {
            assert!(unprocessed.insert(id, std::mem::take(&mut curr)).is_none());
        }
        unprocessed
    };

    let offsets: HashMap<String, usize> = unprocessed
        .keys()
        .enumerate()
        .map(|(i, r)| (r.clone(), i))
        .collect();

    let initial_time = unprocessed["initial_time"][0].1;
    let final_time = unprocessed["final_time"][0].1;
    let saveper = unprocessed["saveper"][0].1;

    let step_size = unprocessed.len();
    let step_count = ((final_time - initial_time) / saveper).ceil() as usize + 1;
    let mut step_data: Vec<f64> = Vec::with_capacity(step_count * step_size);
    step_data.extend(std::iter::repeat(f64::NAN).take(step_count * step_size));

    for (ident, var_off) in offsets.iter() {
        let data = &unprocessed[ident];
        let mut data_iter = data.iter().cloned();
        let mut curr: Option<(f64, f64)> = data_iter.next();
        let mut next: Option<(f64, f64)> = data_iter.next();
        for step in 0..step_count {
            let t: f64 = initial_time + saveper * (step as f64);
            let datapoint: f64 = if let Some((data_time, value)) = curr {
                if approx_eq!(f64, data_time, t) {
                    if next.is_some() {
                        curr = next;
                        next = data_iter.next();
                    }
                    value
                } else {
                    assert!(data_time < t);
                    // curr is in the past
                    if let Some((next_time, next_value)) = next {
                        if approx_eq!(f64, next_time, t) {
                            // next is now now
                            curr = next;
                            next = data_iter.next();
                            next_value
                        } else {
                            // next is still in the future
                            assert!(next_time > t);
                            value
                        }
                    } else {
                        // at the end of the iter, so just use curr
                        value
                    }
                }
            } else {
                unreachable!("curr is None");
            };
            step_data[step * step_size + var_off] = datapoint;
        }
    }

    Ok(Results {
        offsets,
        data: step_data.into_boxed_slice(),
        step_size,
        step_count,
        specs: SimSpecs {
            start: 0.0,
            stop: 0.0,
            dt: 0.0,
            save_step: 0.0,
            method: Method::Euler,
        },
        is_vensim: true,
    })
}

pub fn load_csv(file_path: &str, delimiter: u8) -> StdResult<Results, Box<dyn Error>> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .from_path(file_path)?;

    let header = rdr.headers().unwrap();
    let offsets: HashMap<String, usize> = header
        .iter()
        .enumerate()
        .map(|(i, r)| {
            // stella outputs the first 'time' column as the time _units_, which is bonkers
            let name = if i == 0 { "time" } else { r };
            let ident = canonicalize(name);
            (quoteize(&ident), i)
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
        specs: SimSpecs {
            start: 0.0,
            stop: 0.0,
            dt: 0.0,
            save_step: 0.0,
            method: Method::Euler,
        },
        is_vensim: false,
    })
}
