// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::result::Result as StdResult;

use pico_args::Arguments;

use simlin::errors::{
    FormattedError, FormattedErrorKind, FormattedErrors, format_diagnostic, format_simulation_error,
};
use simlin_engine::common::ErrorKind;
use simlin_engine::datamodel::Project as DatamodelProject;
use simlin_engine::db::{
    PersistentSyncState, SimlinDb, SourceProject, collect_all_diagnostics,
    compile_project_incremental, model_detected_loops, model_module_ident_context,
    parse_source_variable_with_module_context, set_project_ltm_enabled, sync_from_datamodel,
    sync_from_datamodel_incremental,
};
use simlin_engine::prost::Message;
use simlin_engine::{Error, ErrorCode, Result, Results, Vm, datamodel, project_io, serde};
use simlin_engine::{load_csv, load_dat, open_vensim, open_xmile, to_mdl, to_xmile};

mod gen_stdlib;
mod vdf_dump;

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
            "    -h, --help       show this message\n",
            "    --vensim         model is a Vensim .mdl file\n",
            "    --pb-input       input is binary protobuf project\n",
            "    --to-xmile       output should be XMILE not protobuf\n",
            "    --to-mdl         output should be Vensim MDL not protobuf\n",
            "    --model-only     for conversion, only output model instead of project\n",
            "    --output FILE    path to write output file\n",
            "    --reference FILE reference TSV for debug subcommand\n",
            "    --no-output      don't print the output (for benchmarking)\n",
            "    --ltm            enable Loops That Matter analysis\n",
            "    --stdlib-dir DIR directory containing stdlib/*.stmx files\n",
            "\n\
         SUBCOMMANDS:\n",
            "    simulate         Simulate a model and display output\n",
            "    convert          Convert an XMILE or Vensim model to protobuf\n",
            "    equations        Print the equations out\n",
            "    debug            Output model equations interleaved with a reference run\n",
            "    gen-stdlib       Generate Rust code for stdlib models\n",
            "    vdf-dump         Pretty-print VDF file structure and contents\n",
        ),
        VERSION,
        argv0
    );
}

#[derive(Clone, Default, Debug)]
struct Args {
    path: Option<String>,
    output: Option<String>,
    reference: Option<String>,
    stdlib_dir: Option<String>,
    is_vensim: bool,
    is_pb_input: bool,
    is_to_xmile: bool,
    is_to_mdl: bool,
    is_convert: bool,
    is_model_only: bool,
    is_no_output: bool,
    is_equations: bool,
    is_debug: bool,
    is_ltm: bool,
    is_gen_stdlib: bool,
    is_vdf_dump: bool,
}

fn parse_args() -> StdResult<Args, Box<dyn std::error::Error>> {
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
    } else if subcommand == "debug" {
        args.is_debug = true;
    } else if subcommand == "gen-stdlib" {
        args.is_gen_stdlib = true;
    } else if subcommand == "vdf-dump" {
        args.is_vdf_dump = true;
    } else {
        eprintln!("error: unknown subcommand {}", subcommand);
        usage();
    }

    args.output = parsed.value_from_str("--output").ok();
    args.reference = parsed.value_from_str("--reference").ok();
    args.stdlib_dir = parsed.value_from_str("--stdlib-dir").ok();
    args.is_no_output = parsed.contains("--no-output");
    args.is_model_only = parsed.contains("--model-only");
    args.is_to_xmile = parsed.contains("--to-xmile");
    args.is_to_mdl = parsed.contains("--to-mdl");
    args.is_vensim = parsed.contains("--vensim");
    args.is_pb_input = parsed.contains("--pb-input");
    args.is_ltm = parsed.contains("--ltm");

    let free_arguments = parsed.finish();
    if free_arguments.is_empty() && !args.is_gen_stdlib {
        eprintln!("error: input path required");
        usage();
    }

    args.path = free_arguments
        .first()
        .and_then(|s| s.to_str().map(|s| s.to_owned()));

    Ok(args)
}

fn open_binary(reader: &mut dyn BufRead) -> Result<datamodel::Project> {
    let mut contents_buf: Vec<u8> = vec![];
    reader.read_until(0, &mut contents_buf).map_err(|_err| {
        Error::new(
            ErrorKind::Import,
            ErrorCode::VensimConversion,
            Some("1".to_owned()),
        )
    })?;

    let project = match project_io::Project::decode(&*contents_buf) {
        Ok(project) => serde::deserialize(project),
        Err(err) => {
            return Err(Error::new(
                ErrorKind::Import,
                ErrorCode::VensimConversion,
                Some(format!("{err}")),
            ));
        }
    };
    Ok(project)
}

fn print_formatted_error(error: &FormattedError) {
    if matches!(
        error.kind,
        FormattedErrorKind::Variable | FormattedErrorKind::Units
    ) {
        eprintln!();
    }
    if let Some(message) = &error.message {
        eprintln!("{message}");
    }
}

fn report_formatted_errors(formatted: &FormattedErrors) {
    for error in &formatted.errors {
        print_formatted_error(error);
    }
}

/// Collect diagnostics from the salsa accumulator path and convert them
/// to the same `FormattedErrors` structure the CLI has always used.
fn collect_diagnostics_as_formatted(
    db: &SimlinDb,
    source_project: SourceProject,
    sync_state: &PersistentSyncState,
) -> FormattedErrors {
    // Trigger compilation so that diagnostics are accumulated
    let _ = compile_project_incremental(db, source_project, "main");
    let sync = sync_state.to_sync_result();
    let diagnostics = collect_all_diagnostics(db, &sync);
    let mut formatted = FormattedErrors::default();
    for diag in &diagnostics {
        let fe = format_diagnostic(diag);
        if fe.kind == FormattedErrorKind::Variable {
            formatted.has_variable_errors = true;
        }
        if fe.kind == FormattedErrorKind::Model {
            formatted.has_model_errors = true;
        }
        formatted.errors.push(fe);
    }
    formatted
}

fn run_simulation(
    db: &SimlinDb,
    source_project: SourceProject,
    model_name: &str,
) -> StdResult<Results, Error> {
    let compiled = compile_project_incremental(db, source_project, model_name)?;
    let mut vm = Vm::new(compiled)?;
    vm.run_to_end()?;
    Ok(vm.into_results())
}

fn handle_simulation_error(err: &Error, formatted: &FormattedErrors) {
    if err.code == ErrorCode::NotSimulatable && formatted.has_model_errors {
        return;
    }
    let formatted_error = format_simulation_error("main", err);
    print_formatted_error(&formatted_error);
}

fn run_datamodel_with_errors(project: &DatamodelProject) -> Results {
    let mut db = SimlinDb::default();
    let sync_state = sync_from_datamodel_incremental(&mut db, project, None);
    let formatted = collect_diagnostics_as_formatted(&db, sync_state.project, &sync_state);
    report_formatted_errors(&formatted);
    match run_simulation(&db, sync_state.project, "main") {
        Ok(results) => results,
        Err(err) => {
            handle_simulation_error(&err, &formatted);
            die!("failed to create simulation");
        }
    }
}

fn simulate(project: &DatamodelProject, enable_ltm: bool) -> Results {
    if enable_ltm {
        let mut db = SimlinDb::default();
        let sync_state = sync_from_datamodel_incremental(&mut db, project, None);
        let source_project = sync_state.project;

        // Detect and report loops via the salsa path
        let models = source_project.models(&db);
        for (model_name, source_model) in models.iter() {
            if model_name.starts_with("stdlib\u{205A}") {
                continue;
            }
            let detected = model_detected_loops(&db, *source_model, source_project);
            if !detected.loops.is_empty() {
                eprintln!("# Loops in model '{}':", model_name);
                for loop_item in &detected.loops {
                    eprintln!("{} := {}", loop_item.id, loop_item.variables.join(" -> "));
                }
            }
        }

        let formatted = collect_diagnostics_as_formatted(&db, source_project, &sync_state);
        report_formatted_errors(&formatted);

        set_project_ltm_enabled(&mut db, source_project, true);
        match run_simulation(&db, source_project, "main") {
            Ok(results) => return results,
            Err(err) => {
                handle_simulation_error(&err, &formatted);
                eprintln!("Error creating simulation with LTM: {err}");
                eprintln!("falling back to regular simulation without LTM");
            }
        }

        // LTM failed, fall back to non-LTM incremental simulation.
        set_project_ltm_enabled(&mut db, source_project, false);
        match run_simulation(&db, source_project, "main") {
            Ok(results) => return results,
            Err(err) => {
                handle_simulation_error(&err, &formatted);
                die!("failed to create simulation");
            }
        }
    }

    run_datamodel_with_errors(project)
}

fn print_equations(project: &DatamodelProject, output: Option<String>) {
    let mut output_file =
        File::create(output.unwrap_or_else(|| "/dev/stdout".to_string())).unwrap();

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, project);

    let model_names = sync.project.model_names(&db);
    let models = sync.project.models(&db);

    for model_name in model_names.iter() {
        let source_model = match models.get(model_name) {
            Some(m) => *m,
            None => continue,
        };

        // Skip stdlib models (implicitly added for module expansion)
        if model_name.starts_with("stdlib\u{205A}") {
            continue;
        }

        let var_names = source_model.variable_names(&db);
        let vars = source_model.variables(&db);
        let module_ident_context = model_module_ident_context(&db, source_model, vec![]);

        output_file
            .write_fmt(format_args!("% {model_name}\n"))
            .unwrap();
        output_file
            .write_fmt(format_args!("\\begin{{align*}}\n"))
            .unwrap();

        let var_count = var_names.len();
        for (i, var_name) in var_names.iter().enumerate() {
            let source_var = match vars.get(var_name) {
                Some(v) => *v,
                None => continue,
            };

            let parsed = parse_source_variable_with_module_context(
                &db,
                source_var,
                sync.project,
                module_ident_context,
            );
            let var = &parsed.variable;

            let is_stock = var.is_stock();
            let subscript = if is_stock { "(t_0)" } else { "" };
            let display_name = str::replace(var_name.as_str(), "_", "\\_");
            let continuation = if !is_stock && i == var_count - 1 {
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
                    "\\mathrm{{{display_name}}}{subscript} & = {eqn}{continuation}\n"
                ))
                .unwrap();

            if is_stock {
                let inflows = source_var.inflows(&db);
                let outflows = source_var.outflows(&db);
                let continuation = if i == var_count - 1 { "" } else { " \\\\" };
                let use_parens = inflows.len() + outflows.len() > 1;
                let mut flow_eqn = inflows
                    .iter()
                    .map(|inflow| {
                        format!("\\mathrm{{{}}}", str::replace(inflow.as_str(), "_", "\\_"))
                    })
                    .collect::<Vec<_>>()
                    .join(" + ");
                if !outflows.is_empty() {
                    flow_eqn = format!(
                        "{}-{}",
                        flow_eqn,
                        outflows
                            .iter()
                            .map(|outflow| format!(
                                "\\mathrm{{{}}}",
                                str::replace(outflow.as_str(), "_", "\\_")
                            ))
                            .collect::<Vec<_>>()
                            .join(" - ")
                    );
                }
                if use_parens {
                    flow_eqn = format!("({flow_eqn}) ");
                } else {
                    flow_eqn = format!("{flow_eqn} \\cdot ");
                }
                output_file
                    .write_fmt(format_args!(
                        "\\mathrm{{{display_name}}}(t) & = \\mathrm{{{display_name}}}(t - dt) + {flow_eqn} dt{continuation}\n"
                    ))
                    .unwrap();
            }
        }

        output_file
            .write_fmt(format_args!("\\end{{align*}}\n"))
            .unwrap();
    }
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("error: {}", err);
            usage();
        }
    };

    if args.is_gen_stdlib {
        let stdlib_dir = args.stdlib_dir.unwrap_or_else(|| "stdlib".to_string());
        let output_path = args
            .output
            .unwrap_or_else(|| "src/simlin-engine/src/stdlib.gen.rs".to_string());
        if let Err(err) = gen_stdlib::generate(&stdlib_dir, &output_path) {
            die!("gen-stdlib failed: {}", err);
        }
        return;
    }

    if args.is_vdf_dump {
        let file_path = args.path.unwrap_or_else(|| {
            eprintln!("error: VDF file path required");
            std::process::exit(EXIT_FAILURE);
        });
        if let Err(err) = vdf_dump::dump_vdf(&file_path) {
            die!("vdf-dump failed: {}", err);
        }
        return;
    }

    let file_path = args.path.unwrap_or_else(|| "/dev/stdin".to_string());

    let project = if args.is_vensim {
        let contents = std::fs::read_to_string(&file_path).unwrap();
        open_vensim(&contents)
    } else if args.is_pb_input {
        let file = File::open(&file_path).unwrap();
        let mut reader = BufReader::new(file);
        open_binary(&mut reader)
    } else {
        let file = File::open(&file_path).unwrap();
        let mut reader = BufReader::new(file);
        open_xmile(&mut reader)
    };

    if project.is_err() {
        eprintln!("model '{}' error: {}", &file_path, project.err().unwrap());
        return;
    };

    let project = project.unwrap();

    if args.is_equations {
        print_equations(&project, args.output);
    } else if args.is_convert {
        let pb_project = match serde::serialize(&project) {
            Ok(pb) => pb,
            Err(err) => die!("protobuf serialization failed: {}", err),
        };

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
        } else if args.is_to_mdl {
            match to_mdl(&project) {
                Ok(s) => {
                    buf = s.into_bytes();
                }
                Err(err) => {
                    die!("error converting to MDL: {}", err);
                }
            }
        }

        let mut output_file =
            File::create(args.output.unwrap_or_else(|| "/dev/stdout".to_string())).unwrap();
        output_file.write_all(&buf).unwrap();
    } else if args.is_debug {
        if args.reference.is_none() {
            eprintln!("missing required argument --reference FILE");
            std::process::exit(1);
        }
        let ref_path = args.reference.unwrap();
        let reference = if ref_path.ends_with(".dat") {
            load_dat(&ref_path).unwrap()
        } else {
            load_csv(&ref_path, b'\t').unwrap()
        };
        let results = simulate(&project, args.is_ltm);

        results.print_tsv_comparison(Some(&reference));
    } else {
        let results = simulate(&project, args.is_ltm);
        if !args.is_no_output {
            results.print_tsv();
        }
    }
}
