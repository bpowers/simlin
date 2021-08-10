// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::error::Error;
use std::io::BufRead;
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
    use std::io::BufReader;

    use simlin_engine::common::{Error, ErrorCode, ErrorKind};
    use xmutil::convert_vensim_mdl;

    let mut contents_buf: Vec<u8> = vec![];
    reader
        .read_until(0, &mut contents_buf)
        .map_err(|_err| Error::new(ErrorKind::Import, ErrorCode::VensimConversion, None))?;
    let contents: String = String::from_utf8(contents_buf).unwrap();
    let xmile_src: Option<String> = convert_vensim_mdl(&contents, false);
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
    })
}
