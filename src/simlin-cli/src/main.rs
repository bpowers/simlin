// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// mimalloc is this binary's global allocator on native builds: the engine
// compile path is allocation-heavy and mimalloc roughly halves allocator time
// (see docs/design/engine-performance.md). Installed directly here rather than
// via libsimlin so the CLI links no cdylib/staticlib crate -- libsimlin's
// fixed-name (unhashed) rlib otherwise relinked this binary on every
// `cargo build` <-> `cargo build -p simlin-cli` switch. wasm is never a target
// for this binary; the cfg mirrors libsimlin's own allocator guard.
#[cfg(not(target_arch = "wasm32"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::result::Result as StdResult;

use clap::{Args, Parser, Subcommand, ValueEnum};

use simlin_engine::common::ErrorKind;
use simlin_engine::data_provider::FilesystemDataProvider;
use simlin_engine::datamodel::Project as DatamodelProject;
use simlin_engine::db::{
    PersistentSyncState, SimlinDb, SourceProject, collect_all_diagnostics,
    compile_project_incremental, model_detected_loops, parse_source_variable,
    set_project_ltm_enabled, sync_from_datamodel, sync_from_datamodel_incremental,
};
use simlin_engine::errors::{
    FormattedError, FormattedErrorKind, FormattedErrors, collect_formatted_errors,
    format_simulation_error,
};
use simlin_engine::prost::Message;
use simlin_engine::{Error, ErrorCode, Result, Results, build_sim, datamodel, project_io, serde};
use simlin_engine::{
    load_csv, load_dat, open_vensim, open_vensim_with_data, open_xmile, to_mdl_with_warnings,
    to_xmile,
};

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
    /// .mdl -> vensim, .pb/.bin -> protobuf, .txt -> systems, everything else -> xmile)
    #[arg(long, value_enum)]
    format: Option<InputFormat>,
}

#[derive(Clone, Debug, ValueEnum)]
enum InputFormat {
    Xmile,
    Vensim,
    Protobuf,
    Systems,
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
        Some("txt") => InputFormat::Systems,
        _ => InputFormat::Xmile,
    }
}

/// Visible stock metadata for systems format: (original_name, canonical_ident)
/// pairs in declaration order.
type VisibleStocks = Vec<(String, String)>;

/// Load a model file, dispatching on format. Exits on error.
/// For systems format, also returns the visible stocks list (declaration
/// order, original names) for filtered output.
fn open_model(input: &InputArgs) -> (DatamodelProject, Option<VisibleStocks>) {
    let format = resolve_input_format(input);
    let file_path = input
        .path
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/dev/stdin".to_string());

    let (result, visible) = match format {
        InputFormat::Vensim => {
            let contents = std::fs::read_to_string(&file_path).unwrap();
            // `input.path` is the user's intent: `Some` for a named model file
            // (anchors the external-data root), `None` for stdin (pipe or
            // `< file`) where no data root can be inferred.
            (open_vensim_model(input.path.as_deref(), &contents), None)
        }
        InputFormat::Protobuf => {
            let file = File::open(&file_path).unwrap();
            let mut reader = BufReader::new(file);
            (open_binary(&mut reader), None)
        }
        InputFormat::Xmile => {
            let file = File::open(&file_path).unwrap();
            let mut reader = BufReader::new(file);
            (open_xmile(&mut reader), None)
        }
        InputFormat::Systems => {
            let contents = std::fs::read_to_string(&file_path).unwrap();
            let systems_model = simlin_engine::systems::parse(&contents)
                .unwrap_or_else(|e| die!("model '{}' parse error: {}", &file_path, e));
            let visible = simlin_engine::systems::translate::visible_stocks(&systems_model);
            let project = simlin_engine::systems::translate::translate(
                &systems_model,
                simlin_engine::systems::translate::DEFAULT_ROUNDS,
            );
            (project, Some(visible))
        }
    };

    match result {
        Ok(project) => (project, visible),
        Err(err) => die!("model '{}' error: {}", &file_path, err),
    }
}

/// Parse a Vensim MDL model, resolving any `GET DIRECT *` external-data
/// references against the filesystem.
///
/// `path` carries the user's intent, not filesystem metadata:
///
/// - `Some(model_path)` -- the user named a model file on the command line.
///   Its parent directory roots a [`FilesystemDataProvider`], so a relative
///   reference like `GET DIRECT CONSTANTS('data/a.csv', ...)` resolves to a
///   sibling of the model file (matching Vensim's relative-to-model
///   resolution). A bare filename resolves companions against the CWD.
/// - `None` -- the model was read from stdin (whether a pipe or a `< file`
///   redirection). There is no model path to anchor a data root, so no
///   provider is built and any external-data reference remains unresolved;
///   the engine reports a clear "no DataProvider configured" error.
///
/// Keying on the path argument rather than `is_file()` of stdin's
/// `/dev/stdin` sentinel matters: under `simlin simulate < model.mdl`,
/// `/dev/stdin` resolves to a regular file, so an `is_file()` check would
/// wrongly build a provider rooted at `/dev`.
fn open_vensim_model(path: Option<&Path>, contents: &str) -> Result<datamodel::Project> {
    match path {
        Some(model_path) => {
            // A bare filename (e.g. `model.mdl`) has an empty parent; its data
            // companions live in the current directory, so use "." there.
            let base_dir = match model_path.parent() {
                Some(p) if !p.as_os_str().is_empty() => p,
                _ => Path::new("."),
            };
            let provider = FilesystemDataProvider::new(base_dir);
            open_vensim_with_data(contents, Some(&provider))
        }
        // stdin (pipe or `< file`): no model path to anchor a data root.
        None => open_vensim(contents),
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

/// Print TSV output filtered to only the visible stocks, in declaration
/// order with original (non-canonicalized) names. Matches the Python
/// `systems` package behavior.
fn print_filtered_tsv(results: &Results, visible: &VisibleStocks) {
    // Map canonical ident -> offset in the results data
    let ident_to_offset: std::collections::HashMap<&str, usize> = results
        .offsets
        .iter()
        .map(|(k, v)| (k.as_str(), *v))
        .collect();

    // Time is always at offset 0 in the engine's results layout.
    // Skip any visible stock whose canonical ident is "time" to avoid
    // duplicate headers (the time axis is always the first column).
    let mut columns: Vec<(&str, usize)> = vec![("time", 0)];
    for (original_name, canonical_ident) in visible {
        if canonical_ident == "time" {
            continue;
        }
        if let Some(&offset) = ident_to_offset.get(canonical_ident.as_str()) {
            columns.push((original_name.as_str(), offset));
        }
    }

    // Header
    for (col_idx, (name, _)) in columns.iter().enumerate() {
        if col_idx > 0 {
            print!("\t");
        }
        print!("{name}");
    }
    println!();

    // Data rows
    for row in results.iter() {
        if row[0] > results.specs.stop {
            break;
        }
        for (col_idx, (_, offset)) in columns.iter().enumerate() {
            if col_idx > 0 {
                print!("\t");
            }
            print!("{}", row[*offset]);
        }
        println!();
    }
}

/// Print TSV comparison output filtered to only the visible stocks.
/// Each timestep produces two rows: "reference" and "simlin", showing
/// the reference and simulation values side by side for visible stocks only.
fn print_filtered_tsv_comparison(results: &Results, reference: &Results, visible: &VisibleStocks) {
    use simlin_engine::common::{Canonical, Ident};

    // Build columns: (display_name, sim_offset, ref_offset) triples.
    // Time is always at offset 0 in the engine's results layout.
    struct Col<'a> {
        name: &'a str,
        sim_off: usize,
        ref_off: Option<usize>,
    }
    let time_ident = Ident::<Canonical>::from_str_unchecked("time");
    let mut columns: Vec<Col> = vec![Col {
        name: "time",
        sim_off: 0,
        ref_off: reference.offsets.get(&time_ident).copied(),
    }];
    for (original_name, canonical_ident) in visible {
        if canonical_ident == "time" {
            continue;
        }
        let ident = Ident::<Canonical>::from_str_unchecked(canonical_ident);
        if let Some(&sim_off) = results.offsets.get(&ident) {
            let ref_off = reference.offsets.get(&ident).copied();
            columns.push(Col {
                name: original_name.as_str(),
                sim_off,
                ref_off,
            });
        }
    }

    // Header
    print!("series\t");
    for (i, col) in columns.iter().enumerate() {
        if i > 0 {
            print!("\t");
        }
        print!("{}", col.name);
    }
    println!();

    // Data rows
    for (row, ref_row) in results.iter().zip(reference.iter()) {
        if row[0] > results.specs.stop {
            break;
        }
        print!("reference\t");
        for (i, col) in columns.iter().enumerate() {
            if i > 0 {
                print!("\t");
            }
            if let Some(off) = col.ref_off {
                print!("{}", ref_row[off]);
            }
        }
        println!();
        print!("simlin\t");
        for (i, col) in columns.iter().enumerate() {
            if i > 0 {
                print!("\t");
            }
            print!("{}", row[col.sim_off]);
        }
        println!();
    }
}

/// Print one diagnostic to STDERR.
///
/// Everything goes to STDERR, warnings included: STDOUT carries the TSV result
/// table, and a diagnostic interleaved with it would corrupt a redirected or
/// piped run (the same reason the MDL export warnings go to STDERR, #856).
/// `message` already opens with the diagnostic's own severity word, so a
/// warning reads as a warning and `grep '^error'` finds only real errors
/// (GH #919); `error.severity` is here for callers that need to route further.
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
///
/// The datamodel is what makes a diagnostic legible in a terminal:
/// `collect_formatted_errors` renders the offending equation with the span
/// underlined. Without it a parse error prints as a bare `unrecognized_eof` --
/// the one class of error whose whole reason IS the span, so it would be the
/// one class the CLI could never explain.
///
/// `FormattedErrors::push` owns the severity rule, so an advisory `Warning`
/// (a conveyor's LTM-degraded notice, a unit mismatch) is reported to the user
/// but never raises `has_model_errors` / `has_variable_errors`.
fn collect_diagnostics_as_formatted(
    db: &SimlinDb,
    source_project: SourceProject,
    sync_state: &PersistentSyncState,
    project: &DatamodelProject,
) -> FormattedErrors {
    // Trigger compilation so that diagnostics are accumulated
    let _ = compile_project_incremental(db, source_project, "main");
    let sync = sync_state.to_sync_result();
    let diagnostics = collect_all_diagnostics(db, sync.project);
    collect_formatted_errors(&diagnostics, project)
}

fn run_simulation(
    db: &mut SimlinDb,
    source_project: SourceProject,
    project: &DatamodelProject,
    model_name: &str,
) -> StdResult<Results, Error> {
    // `build_sim` routes conveyor/queue models through their special expansion
    // build path and ordinary models through the incremental compile, so the CLI
    // simulates the special stock types instead of tripping the NotExpanded guard.
    let mut vm = build_sim(db, source_project, project, model_name)?;
    vm.run_to_end()?;
    Ok(vm.into_results())
}

/// Whether a VM-build failure is already explained by the diagnostics printed
/// before it.
///
/// `NotSimulatable` is the generic "this model has model-level errors" signal,
/// so it adds nothing once those errors have been printed. It is redundant only
/// when a real *error* was printed: `has_model_errors` counts `Error`-severity
/// diagnostics only, so a model whose sole diagnostic is an advisory warning
/// still reports why its simulation could not be built (GH #919 -- warnings
/// used to set the flag and swallow this).
///
/// pattern: Functional Core -- pure, so the suppression rule is directly testable.
fn simulation_error_is_redundant(err: &Error, formatted: &FormattedErrors) -> bool {
    err.code == ErrorCode::NotSimulatable && formatted.has_model_errors
}

fn handle_simulation_error(err: &Error, formatted: &FormattedErrors) {
    if simulation_error_is_redundant(err, formatted) {
        return;
    }
    let formatted_error = format_simulation_error("main", err);
    print_formatted_error(&formatted_error);
}

fn run_datamodel_with_errors(project: &DatamodelProject) -> Results {
    let mut db = SimlinDb::default();
    let sync_state = sync_from_datamodel_incremental(&mut db, project, None);
    let formatted = collect_diagnostics_as_formatted(&db, sync_state.project, &sync_state, project);
    report_formatted_errors(&formatted);
    match run_simulation(&mut db, sync_state.project, project, "main") {
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

        // Enable LTM BEFORE harvesting diagnostics. `model_all_diagnostics` emits
        // the `ConveyorLtmDegraded`/`QueueLtmDegraded` warnings only inside its
        // `project.ltm_enabled(db)` gate (`db/diagnostic.rs:194`), so collecting
        // first meant a conveyor/queue model silently returned results with no
        // loop scores and no explanation of why. With `--ltm` requested, the
        // LTM-enabled project is the one whose diagnostics the user wants.
        set_project_ltm_enabled(&mut db, source_project, true);

        let formatted = collect_diagnostics_as_formatted(&db, source_project, &sync_state, project);
        report_formatted_errors(&formatted);

        match run_simulation(&mut db, source_project, project, "main") {
            Ok(results) => return results,
            Err(err) => {
                handle_simulation_error(&err, &formatted);
                eprintln!("Error creating simulation with LTM: {err}");
                eprintln!("falling back to regular simulation without LTM");
            }
        }

        // LTM failed, fall back to non-LTM incremental simulation.
        set_project_ltm_enabled(&mut db, source_project, false);
        match run_simulation(&mut db, source_project, project, "main") {
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

            let parsed = parse_source_variable(&db, source_var, sync.project);
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
            let (project, visible) = open_model(&input);
            let results = simulate(&project, ltm);
            if !no_output {
                if let Some(visible) = &visible {
                    print_filtered_tsv(&results, visible);
                } else {
                    results.print_tsv();
                }
            }
        }
        Command::Convert {
            input,
            to,
            model_only,
            output,
        } => {
            let (project, _) = open_model(&input);

            let buf: Vec<u8> = match to {
                OutputFormat::Xmile => match to_xmile(&project) {
                    Ok(s) => {
                        let mut bytes = s.into_bytes();
                        bytes.push(b'\n');
                        bytes
                    }
                    Err(err) => die!("error converting to XMILE: {}", err),
                },
                OutputFormat::Mdl => match to_mdl_with_warnings(&project) {
                    Ok((s, warnings)) => {
                        // Print lossiness warnings to STDERR so they do not
                        // corrupt the MDL when STDOUT is the output file (#856).
                        for w in &warnings {
                            eprintln!("warning: MDL export: {}", w.message);
                        }
                        s.into_bytes()
                    }
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
            let (project, _) = open_model(&input);
            print_equations(&project, output);
        }
        Command::Debug {
            input,
            reference,
            ltm,
        } => {
            let (project, visible) = open_model(&input);
            let ref_path = reference.to_string_lossy();
            let reference_data = if ref_path.ends_with(".dat") {
                load_dat(&ref_path).unwrap()
            } else if ref_path.ends_with(".csv") {
                load_csv(&ref_path, b',').unwrap()
            } else {
                load_csv(&ref_path, b'\t').unwrap()
            };
            let results = simulate(&project, ltm);
            if let Some(visible) = &visible {
                print_filtered_tsv_comparison(&results, &reference_data, visible);
            } else {
                results.print_tsv_comparison(Some(&reference_data));
            }
        }
    }
}

#[cfg(test)]
mod open_vensim_model_tests {
    use super::*;
    use simlin_engine::common::{Canonical, Ident};

    /// Compile and run a datamodel project to completion, returning the
    /// first row of results. Mirrors the CLI's non-LTM `run_simulation`
    /// path so the test exercises the same incremental-salsa pipeline the
    /// `simulate` subcommand uses.
    fn run_to_first_row(project: &DatamodelProject) -> Results {
        let mut db = SimlinDb::default();
        let sync_state = sync_from_datamodel_incremental(&mut db, project, None);
        run_simulation(&mut db, sync_state.project, project, "main")
            .unwrap_or_else(|e| panic!("simulation failed: {e}"))
    }

    fn scalar_value(results: &Results, name: &str) -> f64 {
        let ident = Ident::<Canonical>::new(name);
        let off = results.offsets[&ident];
        results.iter().next().unwrap()[off]
    }

    /// AC5.1/AC5.2: a `GET DIRECT CONSTANTS` model opened through the CLI's
    /// `open_vensim_model` resolves its companion CSV (relative to the model
    /// file) and the resolved value drives simulation.
    ///
    /// `directconst.mdl`'s `a = GET DIRECT CONSTANTS('data/a.csv', ',', 'B2')`
    /// reads cell B2 of `data/a.csv` (`a,\n,2050`), i.e. `2050`. The expected
    /// `directconst.dat` confirms `a -> 2050`. The assertion is tied to the
    /// actual CSV contents, not to zero/NaN.
    #[test]
    fn resolves_get_direct_constants_from_filesystem() {
        let path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/sdeverywhere/models/directconst/directconst.mdl"
        ));
        let contents = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", path.display()));

        let project = open_vensim_model(Some(path), &contents)
            .unwrap_or_else(|e| panic!("open_vensim_model failed: {e}"));

        let results = run_to_first_row(&project);
        let a = scalar_value(&results, "a");
        assert!(
            (a - 2050.0).abs() < 1e-6,
            "a must resolve to the CSV value 2050 (data/a.csv cell B2), got {a}"
        );
    }

    /// AC5.3: a model referencing a missing data file produces a clear,
    /// file-level diagnostic (the `FilesystemDataProvider` "cannot resolve
    /// data file" error surfaced through `open_vensim_model`), NOT a silent
    /// zero or a generic message. The model file is written into a real
    /// (existing) temp directory so the base-directory canonicalize
    /// succeeds and the failure is specifically the missing CSV.
    #[test]
    fn missing_data_file_yields_clear_diagnostic() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let mdl_path = dir.path().join("missing.mdl");
        let mut f = std::fs::File::create(&mdl_path).unwrap();
        write!(
            f,
            "{{UTF-8}}\n\
             a = GET DIRECT CONSTANTS('does_not_exist.csv', ',', 'B2') ~~|\n\
             INITIAL TIME = 0 ~~|\n\
             FINAL TIME = 1 ~~|\n\
             TIME STEP = 1 ~~|\n\
             SAVEPER = TIME STEP ~~|\n"
        )
        .unwrap();
        let contents = std::fs::read_to_string(&mdl_path).unwrap();

        let err = open_vensim_model(Some(&mdl_path), &contents)
            .expect_err("opening a model with a missing data file must error, not silently zero");
        let msg = format!("{err}");
        assert!(
            msg.contains("does_not_exist.csv"),
            "diagnostic must name the offending file; got: {msg}"
        );
        assert!(
            msg.contains("cannot resolve data file"),
            "diagnostic must be the file-level 'cannot resolve data file' error, \
             not a generic message; got: {msg}"
        );
    }

    /// Reading a `GET DIRECT` model from stdin (`path == None`) must NOT build
    /// a `FilesystemDataProvider`: there is no model path to anchor a data
    /// root, whether stdin is a pipe or a `< file` redirection. The null
    /// provider surfaces the engine's "no DataProvider configured" error
    /// rather than silently resolving companions against some unrelated
    /// directory (the `/dev`-rooted provider an `is_file()` check on the
    /// `/dev/stdin` sentinel would have wrongly built under `< file`).
    #[test]
    fn stdin_get_direct_yields_null_provider_error() {
        // Reuse the real `GET DIRECT CONSTANTS` fixture's contents, but feed
        // it through the stdin branch (no path argument).
        let path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/sdeverywhere/models/directconst/directconst.mdl"
        ));
        let contents = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", path.display()));

        let err = open_vensim_model(None, &contents).expect_err(
            "stdin (no path) must not build a data provider, so a GET DIRECT \
             reference must surface the null-provider error",
        );
        let msg = format!("{err}");
        assert!(
            msg.contains("no DataProvider configured"),
            "stdin must use the null provider (no FilesystemDataProvider built); \
             expected the 'no DataProvider configured' error, got: {msg}"
        );
    }

    /// F2 regression: a queue model must simulate through the CLI's
    /// `run_simulation` entry point, not only through `simlin_sim_new`. Before
    /// the `build_sim` dispatch, `compile_project_incremental` hit the
    /// `QueueNotExpanded` guard and `run_simulation` errored on a model the FFI
    /// simulates fine.
    #[test]
    fn simulate_queue_model_via_run_simulation() {
        let xml = include_str!("../../../test/queues/queue_drain.xmile");
        let project = open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
            .expect("parse queue_drain.xmile");
        let results = run_to_first_row(&project);
        assert!(
            results.step_count > 0,
            "queue model must produce simulation results"
        );
    }

    /// F2 regression twin for a conveyor model.
    #[test]
    fn simulate_conveyor_model_via_run_simulation() {
        let xml = include_str!("../../../test/conveyors/minimal_conveyor.xmile");
        let project = open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
            .expect("parse minimal_conveyor.xmile");
        let results = run_to_first_row(&project);
        // The minimal conveyor holds Students at its steady state of 1000.
        assert!(
            (scalar_value(&results, "students") - 1000.0).abs() < 1e-6,
            "conveyor model must simulate to steady state"
        );
    }
}

/// What the CLI actually prints for a diagnostic: the severity word (GH #919 --
/// an advisory `Warning` must not read as a compilation failure) and the source
/// snippet that makes the reason legible.
#[cfg(test)]
mod diagnostic_reporting_tests {
    use super::*;

    /// Run the CLI's diagnostic pass exactly as `simulate(.., enable_ltm)` does,
    /// including the LTM enable that makes the conveyor advisory reachable.
    fn cli_diagnostics(project: &DatamodelProject, enable_ltm: bool) -> FormattedErrors {
        let mut db = SimlinDb::default();
        let sync_state = sync_from_datamodel_incremental(&mut db, project, None);
        let source_project = sync_state.project;
        if enable_ltm {
            set_project_ltm_enabled(&mut db, source_project, true);
        }
        collect_diagnostics_as_formatted(&db, source_project, &sync_state, project)
    }

    fn messages(formatted: &FormattedErrors) -> Vec<String> {
        formatted
            .errors
            .iter()
            .filter_map(|e| e.message.clone())
            .collect()
    }

    /// The issue's reproduce case, at the level the CLI actually decides things:
    /// `--ltm` on a conveyor model emits the `conveyor_ltm_degraded` advisory,
    /// which must render as a warning and leave the error flags clear (so the
    /// run reports success and `handle_simulation_error` stays armed).
    #[test]
    fn conveyor_ltm_advisory_reports_as_a_warning() {
        let xml = include_str!("../../../test/conveyors/minimal_conveyor.xmile");
        let project = open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
            .expect("parse minimal_conveyor.xmile");

        let formatted = cli_diagnostics(&project, true);
        let messages = messages(&formatted);
        assert!(
            messages
                .iter()
                .any(|m| m.contains("conveyor_ltm_degraded") && m.contains("warning in model")),
            "the LTM-degraded advisory must render as a warning: {messages:?}"
        );
        assert!(
            !messages.iter().any(|m| m.contains("error in model")),
            "no diagnostic of this model is an error: {messages:?}"
        );
        assert!(
            !formatted.has_model_errors && !formatted.has_variable_errors,
            "an advisory warning must not raise the error flags"
        );

        // ...and the model still simulates, so the process exits 0. Route
        // through `run_simulation` (which returns a Result) rather than
        // `simulate`, whose failure paths call `die!` -> process::exit and
        // would kill the whole test binary instead of failing this test.
        let mut db = SimlinDb::default();
        let sync_state = sync_from_datamodel_incremental(&mut db, &project, None);
        let results = run_simulation(&mut db, sync_state.project, &project, "main")
            .expect("the LTM run must produce results");
        assert!(results.step_count > 0, "the LTM run must produce results");
    }

    /// The other direction: a genuine equation error still renders as an error
    /// and still raises the variable-error flag.
    #[test]
    fn equation_error_still_reports_as_an_error() {
        let project = simlin_engine::test_common::TestProject::new("cli-error")
            .aux("bad", "1 + bogus", None)
            .build_datamodel();

        let formatted = cli_diagnostics(&project, false);
        let messages = messages(&formatted);
        assert!(
            messages
                .iter()
                .any(|m| m.contains("error in model 'main' variable 'bad'")),
            "a real equation error must render as an error: {messages:?}"
        );
        assert!(
            formatted.has_variable_errors,
            "an Error-severity diagnostic raises the variable-error flag"
        );
    }

    /// A parse error's whole reason is the text under its span -- the raising
    /// site writes no sentence, because the snippet IS the sentence. So the CLI
    /// must render the snippet: the snippet-free formatter prints
    /// `unrecognized_eof` and nothing a modeler can act on.
    #[test]
    fn a_parse_error_prints_the_equation_it_could_not_parse() {
        let project = simlin_engine::test_common::TestProject::new("cli-parse")
            .aux("bad", "1 +", None)
            .build_datamodel();

        let formatted = cli_diagnostics(&project, false);
        let messages = messages(&formatted);
        let parse_message = messages
            .iter()
            .find(|m| m.contains("unrecognized_eof"))
            .unwrap_or_else(|| panic!("expected a parse diagnostic: {messages:?}"));
        assert_eq!(
            parse_message.lines().next(),
            Some("    1 +"),
            "the parse error must lead with the equation it could not parse: {parse_message:?}"
        );
    }

    /// `NotSimulatable` is suppressed only when a real model error explained it.
    /// A warning-only diagnostic set must let the failure through, or the user
    /// is told "failed to create simulation" with no reason at all.
    #[test]
    fn only_real_model_errors_suppress_the_simulation_error() {
        let not_simulatable = Error::new(ErrorKind::Simulation, ErrorCode::NotSimulatable, None);
        let other = Error::new(ErrorKind::Simulation, ErrorCode::CircularDependency, None);

        let clean = FormattedErrors::default();
        assert!(!simulation_error_is_redundant(&not_simulatable, &clean));

        let with_model_error = FormattedErrors {
            has_model_errors: true,
            ..Default::default()
        };
        assert!(simulation_error_is_redundant(
            &not_simulatable,
            &with_model_error
        ));
        assert!(
            !simulation_error_is_redundant(&other, &with_model_error),
            "only the generic NotSimulatable error is redundant"
        );
    }
}
