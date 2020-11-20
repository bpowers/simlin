// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::rc::Rc;

use system_dynamics_engine::{xmile, Project, Simulation};
use xmutil::convert_vensim_mdl;

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

#[derive(Default)]
struct Args {
    path: Option<String>,
    is_vensim: bool,
}

fn parse_args() -> Args {
    let mut args: Args = Default::default();
    for arg in std::env::args().skip(1) {
        if arg == "-help" || arg == "--help" {
            usage();
        } else if arg == "--mdl" {
            args.is_vensim = true;
        } else if arg.chars().next().unwrap_or(' ') == '-' {
            eprintln!("unknown arg '{}'", arg);
            usage();
        } else {
            args.path = Some(arg);
        }
    }

    args
}

fn main() {
    let args = parse_args();
    let file_path = args.path.unwrap_or_else(|| "/dev/stdin".to_string());

    let f = File::open(&file_path).unwrap();
    let project = if args.is_vensim {
        let contents: String = BufReader::new(f)
            .lines()
            .fold("".to_string(), |a, b| a + &b.unwrap());
        let xmile_src: Option<String> = convert_vensim_mdl(&contents, true);
        if xmile_src.is_none() {
            eprintln!("couldn't convert vensim model.\n");
        }
        let xmile_src = xmile_src.unwrap();
        let mut f = BufReader::new(stringreader::StringReader::new(&xmile_src));
        match xmile::project_from_reader(&mut f) {
            Ok(project) => Project::from(project),
            Err(err) => {
                eprintln!("model '{}' error: {}", &file_path, err);
                return;
            }
        }
    } else {
        let mut f = BufReader::new(f);
        match xmile::project_from_reader(&mut f) {
            Ok(project) => Project::from(project),
            Err(err) => {
                eprintln!("model '{}' error: {}", &file_path, err);
                return;
            }
        }
    };

    let project = Rc::new(project);
    let sim = Simulation::new(&project, "main").unwrap();
    let results = sim.run_to_end();
    let results = results.unwrap();
    results.print_tsv();
}
