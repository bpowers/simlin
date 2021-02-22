// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::fs::File;
use std::io::{BufReader, Write};
use std::rc::Rc;

use pico_args::Arguments;

use system_dynamics_compat::engine::datamodel::Equation;
use system_dynamics_compat::engine::{
    eprintln, serde, ErrorCode, Project, Simulation, Variable, VM,
};
use system_dynamics_compat::prost::Message;
use system_dynamics_compat::{open_vensim, open_xmile, to_xmile};

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

fn usage() -> ! {
    let argv0 = std::env::args()
        .next()
        .unwrap_or_else(|| "<mdl>".to_string());
    die!(
        concat!(
            "mdl {}: Simulate system dynamics models.\n\
         \n\
         USAGE:\n",
            "    {} [SUBCOMMAND] [OPTION...] PATH\n",
            "\n\
         OPTIONS:\n",
            "    -h, --help    show this message\n",
            "    --vensim      model is a Vensim .mdl file\n",
            "    --to-xmile    output should be XMILE not protobuf\n",
            "    --model-only  for conversion, only output model instead of project\n",
            "    --output FILE path to write output file\n",
            "    --no-output   don't print the output (for benchmarking)\n",
            "\n\
         SUBCOMMANDS:\n",
            "    simulate      Simulate a model and display output\n",
            "    convert       Convert an XMILE or Vensim model to protobuf\n",
            "    equations     Print the equations out\n",
        ),
        VERSION,
        argv0
    );
}

#[derive(Clone, Default, Debug)]
struct Args {
    path: Option<String>,
    output: Option<String>,
    is_vensim: bool,
    is_to_xmile: bool,
    is_convert: bool,
    is_model_only: bool,
    is_no_output: bool,
    is_equations: bool,
}

fn parse_args() -> Result<Args, Box<dyn std::error::Error>> {
    let mut parsed = Arguments::from_env();
    if parsed.contains(["-h", "--help"]) {
        usage();
    }

    let subcommand = parsed.subcommand()?;
    if subcommand.is_none() {
        eprintln!("error: subcommand required");
        usage();
    }

    let mut args: Args = Default::default();

    let subcommand = subcommand.unwrap();
    if subcommand == "convert" {
        args.is_convert = true;
    } else if subcommand == "simulate" {
    } else if subcommand == "equations" {
        args.is_equations = true;
    } else {
        eprintln!("error: unknown subcommand {}", subcommand);
        usage();
    }

    args.output = parsed.value_from_str("--output").ok();
    args.is_no_output = parsed.contains("--no-output");
    args.is_model_only = parsed.contains("--model-only");
    args.is_to_xmile = parsed.contains("--to-xmile");
    args.is_vensim = parsed.contains("--vensim");

    let free_arguments = parsed.free()?;
    if free_arguments.is_empty() {
        eprintln!("error: input path required");
        usage();
    }

    args.path = Some(free_arguments[0].clone());

    Ok(args)
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("error: {}", err);
            usage();
        }
    };
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

    if args.is_equations {
        let mut output_file =
            File::create(&args.output.unwrap_or_else(|| "/dev/stdout".to_string())).unwrap();

        let project = Rc::new(Project::from(project));
        for (model_name, model) in project.models.iter().filter(|(_, model)| !model.implicit) {
            output_file
                .write_fmt(format_args!("% {}\n", model_name))
                .unwrap();
            output_file
                .write_fmt(format_args!("\\begin{{align*}}\n"))
                .unwrap();

            let var_count = model.variables.len();
            for (i, (var_name, var)) in model.variables.iter().enumerate() {
                let subscript = if var.is_stock() { "(t_0)" } else { "" };
                let var_name = str::replace(var_name, "_", "\\_");
                let continuation = if !var.is_stock() && i == var_count - 1 {
                    ""
                } else {
                    " \\\\"
                };
                let eqn = var
                    .ast()
                    .map(|ast| ast.to_latex())
                    .unwrap_or_else(|| "\\varnothing".to_owned());
                output_file
                    .write_fmt(format_args!(
                        "\\mathrm{{{}}}{} & = {}{}\n",
                        var_name, subscript, eqn, continuation
                    ))
                    .unwrap();

                if var.is_stock() {
                    if let Variable::Stock {
                        inflows, outflows, ..
                    } = var
                    {
                        let continuation = if i == var_count - 1 { "" } else { " \\\\" };
                        let use_parens = inflows.len() + outflows.len() > 1;
                        let mut eqn = inflows
                            .iter()
                            .map(|inflow| {
                                format!("\\mathrm{{{}}}", str::replace(inflow, "_", "\\_"))
                            })
                            .collect::<Vec<_>>()
                            .join(" + ");
                        if !outflows.is_empty() {
                            eqn = format!(
                                "{}-{}",
                                eqn,
                                outflows
                                    .iter()
                                    .map(|inflow| format!(
                                        "\\mathrm{{{}}}",
                                        str::replace(inflow, "_", "\\_")
                                    ))
                                    .collect::<Vec<_>>()
                                    .join(" - ")
                            );
                        }
                        if use_parens {
                            eqn = format!("({}) ", eqn);
                        } else {
                            eqn = format!("{} \\cdot ", eqn);
                        }
                        output_file
                            .write_fmt(format_args!(
                                "\\mathrm{{{}}}(t) & = \\mathrm{{{}}}(t - dt) + {} dt{}\n",
                                var_name, var_name, eqn, continuation
                            ))
                            .unwrap();
                    }
                }
            }

            output_file
                .write_fmt(format_args!("\\end{{align*}}\n"))
                .unwrap();
        }
    } else if args.is_convert {
        let pb_project = serde::serialize(&project);

        let mut buf: Vec<u8> = if args.is_model_only {
            if pb_project.models.len() != 1 {
                die!("--model-only specified, but more than 1 model in this project");
            }
            let mut buf = Vec::with_capacity(pb_project.models[0].encoded_len());
            pb_project.models[0].encode(&mut buf).unwrap();
            buf
        } else {
            let mut buf = Vec::with_capacity(pb_project.encoded_len());
            pb_project.encode(&mut buf).unwrap();
            buf
        };

        if args.is_to_xmile {
            match to_xmile(&project) {
                Ok(s) => {
                    buf = s.into_bytes();
                    buf.push(b'\n');
                }
                Err(err) => {
                    die!("error converting to XMILE: {}", err);
                }
            }
        }

        let mut output_file =
            File::create(&args.output.unwrap_or_else(|| "/dev/stdout".to_string())).unwrap();
        output_file.write_all(&buf).unwrap();
    // eprintln!("{:?}", project);
    } else {
        let project_datamodel = project.clone();
        let project = Rc::new(Project::from(project));
        let mut found_model_error = false;
        for (model_name, model) in project.models.iter() {
            let model_datamodel = project_datamodel.get_model(model_name);
            if model_datamodel.is_none() {
                continue;
            }
            let model_datamodel = model_datamodel.unwrap();
            let mut found_var_error = false;
            for (ident, errors) in model.get_variable_errors() {
                assert!(!errors.is_empty());
                let var = model_datamodel.get_variable(&ident).unwrap();
                found_var_error = true;
                for error in errors {
                    eprintln!();
                    if let Some(Equation::Scalar(eqn)) = var.get_equation() {
                        eprintln!("    {}", eqn);
                        let space = std::iter::repeat(" ")
                            .take(error.start as usize)
                            .collect::<String>();
                        let underline = std::iter::repeat("~")
                            .take((error.end - error.start) as usize)
                            .collect::<String>();
                        eprintln!("    {}{}", space, underline);
                    }
                    eprintln!(
                        "error in model '{}' variable '{}': {}",
                        model_name, ident, error.code
                    );
                }
            }
            if let Some(errors) = &model.errors {
                for error in errors.iter() {
                    if error.code == ErrorCode::VariablesHaveErrors && found_var_error {
                        continue;
                    }
                    eprintln!("error in model {}: {}", model_name, error);
                    found_model_error = true;
                }
            }
        }
        let sim = match Simulation::new(&project, "main") {
            Ok(sim) => sim,
            Err(err) => {
                if !(err.code == ErrorCode::NotSimulatable && found_model_error) {
                    eprintln!("error: {}", err);
                }
                std::process::exit(1);
            }
        };
        let compiled = sim.compile().unwrap();
        let mut vm = VM::new(compiled).unwrap();
        vm.run_to_end().unwrap();
        let results = vm.into_results();
        if !args.is_no_output {
            results.print_tsv();
        }
    }
}
