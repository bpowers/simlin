// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::result::Result as StdResult;

use clap::{Args, Parser, Subcommand, ValueEnum};

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

const EXIT_FAILURE: i32 = 1;

#[macro_export]
macro_rules! die(
    ($($arg:tt)*) => { {
        use std;
        eprintln!($($arg)*);
        std::process::exit(EXIT_FAILURE)
    } }
);

#[derive(Debug, Parser)]
#[command(name = "simlin", version, about = "Simulate system dynamics models")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Simulate a model and print results as TSV
    Simulate {
        #[command(flatten)]
        input: InputArgs,

        /// Suppress output (useful for benchmarking)
        #[arg(long)]
        no_output: bool,

        /// Enable Loops That Matter analysis
        #[arg(long)]
        ltm: bool,
    },

    /// Convert a model between formats
    Convert {
        #[command(flatten)]
        input: InputArgs,

        /// Output format (defaults to protobuf)
        #[arg(long, value_enum, default_value_t = OutputFormat::Protobuf)]
        to: OutputFormat,

        /// Output only the model, not the full project (protobuf only)
        #[arg(long)]
        model_only: bool,

        /// Output file path (defaults to stdout)
        #[arg(long, short)]
        output: Option<PathBuf>,
    },

    /// Print model equations as LaTeX
    Equations {
        #[command(flatten)]
        input: InputArgs,

        /// Output file path (defaults to stdout)
        #[arg(long, short)]
        output: Option<PathBuf>,
    },

    /// Compare simulation output with a reference run
    Debug {
        #[command(flatten)]
        input: InputArgs,

        /// Reference TSV or DAT file for comparison
        #[arg(long)]
        reference: PathBuf,

        /// Enable Loops That Matter analysis
        #[arg(long)]
        ltm: bool,
    },

    /// Generate Rust code for stdlib models
    GenStdlib {
        /// Directory containing stdlib .stmx files
        #[arg(long, default_value = "stdlib")]
        stdlib_dir: PathBuf,

        /// Output file path
        #[arg(long, short, default_value = "src/simlin-engine/src/stdlib.gen.rs")]
        output: PathBuf,
    },

    /// Pretty-print VDF file structure and contents
    VdfDump {
        /// VDF file path
        path: PathBuf,
    },
}

/// Shared arguments for commands that read a model file.
#[derive(Clone, Debug, Args)]
struct InputArgs {
    /// Model file path (reads stdin if omitted)
    path: Option<PathBuf>,

    /// Input format (auto-detected from file extension when omitted:
    /// .mdl -> vensim, .pb/.bin -> protobuf, everything else -> xmile)
    #[arg(long, value_enum)]
    format: Option<InputFormat>,
}

#[derive(Clone, Debug, ValueEnum)]
enum InputFormat {
    Xmile,
    Vensim,
    Protobuf,
}

#[derive(Clone, Debug, ValueEnum)]
enum OutputFormat {
    Protobuf,
    Xmile,
    Mdl,
}

/// Infer input format from file extension, falling back to XMILE.
fn resolve_input_format(input: &InputArgs) -> InputFormat {
    if let Some(fmt) = &input.format {
        return fmt.clone();
    }
    match input
        .path
        .as_ref()
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())
    {
        Some("mdl") => InputFormat::Vensim,
        Some("pb" | "bin") => InputFormat::Protobuf,
        _ => InputFormat::Xmile,
    }
}

/// Load a model file, dispatching on format. Exits on error.
fn open_model(input: &InputArgs) -> DatamodelProject {
    let format = resolve_input_format(input);
    let file_path = input
        .path
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/dev/stdin".to_string());

    let result = match format {
        InputFormat::Vensim => {
            let contents = std::fs::read_to_string(&file_path).unwrap();
            open_vensim(&contents)
        }
        InputFormat::Protobuf => {
            let file = File::open(&file_path).unwrap();
            let mut reader = BufReader::new(file);
            open_binary(&mut reader)
        }
        InputFormat::Xmile => {
            let file = File::open(&file_path).unwrap();
            let mut reader = BufReader::new(file);
            open_xmile(&mut reader)
        }
    };

    match result {
        Ok(project) => project,
        Err(err) => die!("model '{}' error: {}", &file_path, err),
    }
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

fn print_equations(project: &DatamodelProject, output: Option<PathBuf>) {
    let output_path = output.unwrap_or_else(|| PathBuf::from("/dev/stdout"));
    let mut output_file = File::create(&output_path).unwrap();

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, project);

    let model_names = sync.project.model_names(&db);
    let models = sync.project.models(&db);

    for model_name in model_names.iter() {
        let canonical_name = simlin_engine::canonicalize(model_name);
        let source_model = match models.get(canonical_name.as_ref()) {
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
    let cli = Cli::parse();

    match cli.command {
        Command::GenStdlib { stdlib_dir, output } => {
            if let Err(err) =
                gen_stdlib::generate(&stdlib_dir.to_string_lossy(), &output.to_string_lossy())
            {
                die!("gen-stdlib failed: {}", err);
            }
        }
        Command::VdfDump { path } => {
            if let Err(err) = vdf_dump::dump_vdf(&path.to_string_lossy()) {
                die!("vdf-dump failed: {}", err);
            }
        }
        Command::Simulate {
            input,
            no_output,
            ltm,
        } => {
            let project = open_model(&input);
            let results = simulate(&project, ltm);
            if !no_output {
                results.print_tsv();
            }
        }
        Command::Convert {
            input,
            to,
            model_only,
            output,
        } => {
            let project = open_model(&input);

            let buf: Vec<u8> = match to {
                OutputFormat::Xmile => match to_xmile(&project) {
                    Ok(s) => {
                        let mut bytes = s.into_bytes();
                        bytes.push(b'\n');
                        bytes
                    }
                    Err(err) => die!("error converting to XMILE: {}", err),
                },
                OutputFormat::Mdl => match to_mdl(&project) {
                    Ok(s) => s.into_bytes(),
                    Err(err) => die!("error converting to MDL: {}", err),
                },
                OutputFormat::Protobuf => {
                    let pb_project = match serde::serialize(&project) {
                        Ok(pb) => pb,
                        Err(err) => die!("protobuf serialization failed: {}", err),
                    };
                    if model_only {
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
                    }
                }
            };

            let output_path = output.unwrap_or_else(|| PathBuf::from("/dev/stdout"));
            let mut output_file = File::create(&output_path).unwrap();
            output_file.write_all(&buf).unwrap();
        }
        Command::Equations { input, output } => {
            let project = open_model(&input);
            print_equations(&project, output);
        }
        Command::Debug {
            input,
            reference,
            ltm,
        } => {
            let project = open_model(&input);
            let ref_path = reference.to_string_lossy();
            let reference_data = if ref_path.ends_with(".dat") {
                load_dat(&ref_path).unwrap()
            } else {
                load_csv(&ref_path, b'\t').unwrap()
            };
            let results = simulate(&project, ltm);
            results.print_tsv_comparison(Some(&reference_data));
        }
    }
}
