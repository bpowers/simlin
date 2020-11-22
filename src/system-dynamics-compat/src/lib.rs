// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::io::BufRead;

pub use system_dynamics_engine as engine;
use system_dynamics_engine::datamodel::Project;
pub use system_dynamics_engine::prost;
use system_dynamics_engine::Result;

pub mod xmile;

#[cfg(feature = "vensim")]
pub fn open_vensim(reader: &mut dyn BufRead) -> Result<Project> {
    use std::io::BufReader;
    use xmutil::convert_vensim_mdl;

    let contents: String = reader.lines().fold("".to_string(), |a, b| a + &b.unwrap());
    let xmile_src: Option<String> = convert_vensim_mdl(&contents, true);
    if xmile_src.is_none() {
        eprintln!("couldn't convert vensim model.\n");
    }
    let xmile_src = xmile_src.unwrap();
    let mut f = BufReader::new(xmile_src.as_bytes());
    xmile::project_from_reader(&mut f)
}

pub fn open_xmile(reader: &mut dyn BufRead) -> Result<Project> {
    xmile::project_from_reader(reader)
}
