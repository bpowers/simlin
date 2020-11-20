// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::rc::Rc;

use clap::{App, Arg, SubCommand};

use system_dynamics_engine::{datamodel, eprintln, xmile, Project, Result, Simulation};
use xmutil::convert_vensim_mdl;

const VERSION: &str = "1.0";

#[macro_export]
macro_rules! die(
    ($($arg:tt)*) => { {
        use std;
        eprintln!($($arg)*);
        std::process::exit(EXIT_FAILURE)
    } }
);

#[derive(Clone, Default, Debug)]
struct Args {
    path: Option<String>,
    output: Option<String>,
    is_vensim: bool,
    is_convert: bool,
}

fn parse_args() -> Args {
    let matches = App::new("mdl")
        .version(VERSION)
        .author("Bobby Powers <bobbypowers@gmail.com>")
        .about("Simulate and convert XMILE and Vensim system dynamics models")
        .arg(
            Arg::with_name("INPUT")
                .help("Input model path")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::with_name("vensim")
                .long("vensim")
                .help("Sets a custom config file"),
        )
        .subcommand(
            SubCommand::with_name("convert").about("Convert ").arg(
                Arg::with_name("output")
                    .short("o")
                    .long("output")
                    .value_name("OUTPUT")
                    .help("Output protobuf-encoded model path")
                    .takes_value(true),
            ),
        )
        .get_matches();

    let mut args: Args = Default::default();
    args.path = matches.value_of("INPUT").map(|p| p.to_string());
    args.is_vensim = matches.value_of("vensim").is_some();
    if matches.subcommand_name() == Some("convert") {
        args.is_convert = true;
        let matches = matches.subcommand_matches("convert").unwrap();
        args.output = matches.value_of("output").map(|p| p.to_string())
    }

    args
}

fn open_vensim(file: &File) -> Result<datamodel::Project> {
    let contents: String = BufReader::new(file)
        .lines()
        .fold("".to_string(), |a, b| a + &b.unwrap());
    let xmile_src: Option<String> = convert_vensim_mdl(&contents, true);
    if xmile_src.is_none() {
        eprintln!("couldn't convert vensim model.\n");
    }
    let xmile_src = xmile_src.unwrap();
    let mut f = BufReader::new(stringreader::StringReader::new(&xmile_src));
    xmile::project_from_reader(&mut f)
}

fn open_xmile(file: &File) -> Result<datamodel::Project> {
    let mut f = BufReader::new(file);
    xmile::project_from_reader(&mut f)
}

fn main() {
    let args = parse_args();
    eprintln!("args: {:?}", args);

    let file_path = args.path.unwrap_or_else(|| "/dev/stdin".to_string());
    let file = File::open(&file_path).unwrap();

    let project = if args.is_vensim {
        open_vensim(&file)
    } else {
        open_xmile(&file)
    };

    if project.is_err() {
        eprintln!("model '{}' error: {}", &file_path, project.err().unwrap());
        return;
    };

    if args.is_convert {
        eprintln!("TODO: convert");

        return;
    }

    let project = Rc::new(Project::from(project.unwrap()));
    let sim = Simulation::new(&project, "main").unwrap();
    let results = sim.run_to_end();
    let results = results.unwrap();
    results.print_tsv();
}
