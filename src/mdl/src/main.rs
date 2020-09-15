// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use engine_core::{self, Simulation};
use std::fs::File;
use std::io::BufReader;

const VERSION: &str = "1.0";
const EXIT_FAILURE: i32 = 1;

// from https://stackoverflow.com/questions/27588416/how-to-send-output-to-stderr
#[macro_export]
macro_rules! eprintln(
    ($($arg:tt)*) => { {
        use std::io::Write;
        let r = writeln!(&mut ::std::io::stderr(), $($arg)*);
        r.expect("failed printing to stderr");
    } }
);

#[macro_export]
macro_rules! die(
    ($($arg:tt)*) => { {
        use std;
        eprintln!($($arg)*);
        std::process::exit(EXIT_FAILURE)
    } }
);

fn usage() -> ! {
    let argv0 = std::env::args()
        .next()
        .unwrap_or_else(|| "<mdl>".to_string());
    die!(
        "Usage: {} [OPTION...] PATH\n\
          mdl {}: Simulate system dynamics models.\n\
          \n\
          Options:\n\
            -help:\tshow this message",
        VERSION,
        argv0
    );
}

fn parse_args() -> Option<String> {
    for arg in std::env::args().skip(1) {
        if arg == "-help" || arg == "--help" {
            usage();
        } else if arg.chars().next().unwrap_or(' ') == '-' {
            eprintln!("unknown arg '{}'", arg);
            usage();
        } else {
            return Some(arg);
        }
    }

    // no args? reading from stdin then
    None
}

fn main() {
    let file_path = parse_args().unwrap_or_else(|| "/dev/stdin".to_string());

    let f = File::open(&file_path).unwrap();
    let mut f = BufReader::new(f);

    let project = engine_core::Project::from_xmile_reader(&mut f);
    if let Err(ref err) = project {
        eprintln!("model '{}' error: {}", &file_path, err);
    }
    assert!(project.is_ok());

    let project = project.unwrap();
    let model = project.models.get("main").unwrap().clone();
    let sim = Simulation::new(&project, model).unwrap();
    let results = sim.run_to_end();
    let results = results.unwrap();
    results.print_tsv();
}
