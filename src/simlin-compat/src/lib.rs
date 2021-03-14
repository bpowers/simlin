// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::io::BufRead;

use simlin_engine::datamodel::Project;
pub use simlin_engine::{self as engine, prost, Result};

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
