// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use clap::{App, Arg};
use engine_core::{self, Simulation};
use std::fs::File;
use std::io::BufReader;

fn main() {
    let matches = App::new("mdl")
        .version("1.0")
        .author("Bobby Powers <bobbypowers@gmail.com>")
        .about("Simulate system dynamics models")
        .arg(
            Arg::new("INPUT")
                .about("XMILE file to simulate")
                .required(true)
                .index(1),
        )
        .get_matches();

    let file_path = matches.value_of("INPUT").unwrap();

    let f = File::open(file_path).unwrap();
    let mut f = BufReader::new(f);

    let project = engine_core::Project::from_xmile_reader(&mut f);
    if let Err(ref err) = project {
        eprintln!("model '{}' error: {}", file_path, err);
    }
    assert!(project.is_ok());

    let project = project.unwrap();
    let model = project.models.get("main").unwrap().clone();
    let sim = Simulation::new(&project, model).unwrap();
    let results = sim.run_to_end();
    let results = results.unwrap();
    results.print_tsv();
}
