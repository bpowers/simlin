// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::fs::File;
use std::io::{BufReader, Write};
use std::rc::Rc;

use clap::{App, Arg, SubCommand};

use system_dynamics_compat::engine::{eprintln, serde, Project, Simulation};
use system_dynamics_compat::prost::Message;
use system_dynamics_compat::{open_vensim, open_xmile};

const VERSION: &str = "1.0";
const EXIT_FAILURE: i32 = 1;

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
    is_model_only: bool,
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
            SubCommand::with_name("convert")
                .about("Convert ")
                .arg(
                    Arg::with_name("output")
                        .short("o")
                        .long("output")
                        .value_name("OUTPUT")
                        .help("Output protobuf-encoded model path")
                        .takes_value(true)
                        .required(true),
                )
                .arg(
                    Arg::with_name("model-only")
                        .long("model-only")
                        .help("Only output the model, not the project"),
                ),
        )
        .get_matches();

    let mut args: Args = Default::default();
    args.path = matches.value_of("INPUT").map(|p| p.to_string());
    args.is_vensim = matches.is_present("vensim");
    if matches.subcommand_name() == Some("convert") {
        args.is_convert = true;
        let matches = matches.subcommand_matches("convert").unwrap();
        args.output = matches.value_of("output").map(|p| p.to_string());
        args.is_model_only = matches.is_present("model-only");
    }

    args
}

fn main() {
    let args = parse_args();
    let file_path = args.path.unwrap_or_else(|| "/dev/stdin".to_string());
    let file = File::open(&file_path).unwrap();
    let mut reader = BufReader::new(file);

    let project = if args.is_vensim {
        open_vensim(&mut reader)
    } else {
        open_xmile(&mut reader)
    };

    if project.is_err() {
        eprintln!("model '{}' error: {}", &file_path, project.err().unwrap());
        return;
    };

    let project = project.unwrap();

    if args.is_convert {
        let pb_project = serde::serialize(&project);

        let buf: Vec<u8> = if args.is_model_only {
            if pb_project.models.len() != 1 {
                die!("--model-only specified, but more than 1 model in this project");
            }
            let mut buf = Vec::with_capacity(pb_project.models[0].encoded_len() + 8);
            pb_project.models[0]
                .encode_length_delimited(&mut buf)
                .unwrap();
            buf
        } else {
            let mut buf = Vec::with_capacity(pb_project.encoded_len() + 8);
            pb_project.encode_length_delimited(&mut buf).unwrap();
            buf
        };

        let mut output_file = File::create(&args.output.unwrap()).unwrap();
        output_file.write_all(&buf).unwrap();
    } else {
        let project = Rc::new(Project::from(project));
        let sim = Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end();
        let results = results.unwrap();
        results.print_tsv();
    }
}
