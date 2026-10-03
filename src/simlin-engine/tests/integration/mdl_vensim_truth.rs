// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The MDL reader against Vensim: every corpus `.mdl` with Vensim's own
//! output beside it is imported by the native reader, simulated, and compared
//! with that output.
//!
//! `simulate.rs` runs most of `test/test-models` through the XMILE twin of
//! each model, which says nothing about what the MDL reader makes of the
//! `.mdl`. The gates that do read the `.mdl` files (the save fixed point, the
//! save-meaning check) compare Simlin with Simlin, so a reader that is wrong
//! is wrong on both sides and passes. This gate is the one that holds the
//! reader to ground truth.
//!
//! It is a ratchet. `EXPECTED` lists every file that does not match, by how
//! it fails and why. A file that stops matching fails the gate, and so does a
//! listed file whose class changes, so a fix removes or reclassifies the
//! entries it repairs. Within a file that differs, `mdl_vensim_truth_series.tsv`
//! lists the series that differ, so a new difference in a file that already
//! has some fails the gate too, as does a listed series that comes to match
//! (`UPDATE_VENSIM_TRUTH_SERIES=1` rewrites the table from a run). It is the
//! oracle for the reader: a change to how a file imports is right when it
//! moves no file out of `Matches` and no series into a difference.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use simlin_engine::db::{
    DiagnosticSeverity, LtmOverlay, SimlinDb, collect_all_diagnostics, compile_project_incremental,
    sync_from_datamodel_incremental,
};
use simlin_engine::{FilesystemDataProvider, Results, Vm, load_csv, load_dat};

use crate::test_helpers::series_that_differ;

/// How an import compares with Vensim's output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Class {
    /// Every series Vensim saved matches.
    Matches,
    /// The model simulates and some series differs.
    Differs,
    /// The model imports and does not compile or run.
    DoesNotSimulate,
    /// The reader refuses the file.
    DoesNotImport,
}

impl Class {
    const ALL: [Class; 4] = [
        Class::Matches,
        Class::Differs,
        Class::DoesNotSimulate,
        Class::DoesNotImport,
    ];
}

/// Files larger than this are left out so the sweep stays quick; only C-LEARN
/// is, and `simulate.rs` holds it to its own reference run.
const MAX_BYTES: u64 = 200 * 1024;

/// Fewer comparisons than this means the corpus walk or the output loaders
/// broke, which would otherwise pass the gate vacuously.
const MIN_COMPARED: usize = 180;

/// Every corpus `.mdl` with a Vensim output beside it, sorted, with that
/// output: `output.csv` or `output.tab` when the directory holds one model,
/// else the model's own `.dat`.
fn corpus() -> Vec<(String, Results)> {
    let root = PathBuf::from("../..");
    let mut files = Vec::new();
    let mut dirs = vec![root.join("test")];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        let is_mdl = |p: &PathBuf| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("mdl"))
        };
        let models_here = paths.iter().filter(|p| is_mdl(p)).count();
        for path in paths {
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if !is_mdl(&path) || !fs::metadata(&path).is_ok_and(|m| m.len() <= MAX_BYTES) {
                continue;
            }
            if let Some(expected) = vensim_output(&path, models_here == 1) {
                let rel = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                files.push((rel, expected));
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

fn vensim_output(mdl: &Path, alone: bool) -> Option<Results> {
    let dir = mdl.parent()?;
    let shared = [("output.csv", b','), ("output.tab", b'\t')]
        .into_iter()
        .map(|(name, delimiter)| (dir.join(name), delimiter))
        .find(|(path, _)| alone && path.exists());
    let mut results = match shared {
        Some((path, delimiter)) => load_csv(&path.to_string_lossy(), delimiter).ok()?,
        None => {
            let dat = mdl.with_extension("dat");
            if !dat.exists() {
                return None;
            }
            load_dat(&dat.to_string_lossy()).ok()?
        }
    };
    results.is_vensim = true;
    Some(results)
}

/// How the import of `path` (from the checkout's root) compares with
/// `expected`, a line saying what was found, and for a file that differs the
/// series that differ.
pub(crate) fn classify(path: &str, expected: &Results) -> (Class, String, Vec<String>) {
    let file = PathBuf::from("../..").join(path);
    let Ok(text) = fs::read_to_string(&file) else {
        return (
            Class::DoesNotImport,
            "the file is not UTF-8".to_string(),
            Vec::new(),
        );
    };
    let dir = file.parent().unwrap_or(Path::new("."));
    let provider = FilesystemDataProvider::new(dir);
    let project = match simlin_engine::open_vensim_with_data(&text, Some(&provider)) {
        Ok(project) => project,
        Err(err) => return (Class::DoesNotImport, err.to_string(), Vec::new()),
    };
    let run = || -> Result<Results, String> {
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        let compiled = compile_project_incremental(&db, sync.project, "main", LtmOverlay::Off)
            .map_err(|_| {
                // The errors themselves say more than the compile's summary.
                collect_all_diagnostics(&db, sync.project, LtmOverlay::Off)
                    .iter()
                    .filter(|d| d.severity == DiagnosticSeverity::Error)
                    .take(3)
                    .map(|d| {
                        format!(
                            "{}: {} ({})",
                            d.variable.as_deref().unwrap_or("the model"),
                            d.code(),
                            d.reason().unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            })?;
        let mut vm = Vm::new(compiled).map_err(|e| e.to_string())?;
        vm.run_to_end().map_err(|e| e.to_string())?;
        Ok(vm.into_results())
    };
    let results = match run() {
        Ok(results) => results,
        Err(err) => return (Class::DoesNotSimulate, err, Vec::new()),
    };
    // `series_that_differ` is the comparison every corpus test asserts
    // (`ensure_results`), as a list.
    if expected.step_count != results.step_count {
        return (
            Class::Differs,
            first_difference(expected, &results),
            vec![STEP_COUNT.to_owned()],
        );
    }
    let differ: Vec<String> = series_that_differ(expected, &results, &[])
        .into_iter()
        .map(|(series, _)| series)
        .collect();
    if differ.is_empty() {
        (Class::Matches, String::new(), differ)
    } else {
        (Class::Differs, first_difference(expected, &results), differ)
    }
}

/// The series a run that saves another number of steps than Vensim is
/// listed as differing in: no series compares.
const STEP_COUNT: &str = "<the number of saved steps>";

/// The series that differ in each file that differs, `path\tseries` per line,
/// sorted.
const DIFFERING_SERIES: &str = include_str!("mdl_vensim_truth_series.tsv");

fn differing_series() -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in DIFFERING_SERIES.lines().filter(|l| !l.trim().is_empty()) {
        let (path, series) = line.split_once('\t').unwrap_or((line, ""));
        out.entry(path.to_owned())
            .or_default()
            .push(series.to_owned());
    }
    out
}

const CONTROL_SERIES: &[&str] = &["saveper", "initial_time", "final_time", "time_step"];

/// The first series of `expected` that `results` lacks or holds another
/// value for, for a failure's message; `ensure_results` decides whether the
/// two differ.
fn first_difference(expected: &Results, results: &Results) -> String {
    if expected.step_count != results.step_count {
        return format!(
            "Vensim saved {} steps, Simlin {}",
            expected.step_count, results.step_count
        );
    }
    let mut names: Vec<_> = expected.offsets.iter().collect();
    names.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    for (step, (want_row, got_row)) in expected.iter().zip(results.iter()).enumerate() {
        for (name, at) in &names {
            let want = want_row[**at];
            let Some(&off) = results.offsets.get(*name) else {
                // The control variables and Vensim's internal delay levels
                // are series `ensure_results` does not ask for.
                let asked_for =
                    !CONTROL_SERIES.contains(&name.as_str()) && !name.as_str().starts_with('#');
                if step == 0 && asked_for {
                    return format!("no series {name}");
                }
                continue;
            };
            let got = got_row[off];
            let scale = want.abs().max(got.abs()).max(1.0);
            if want.is_nan() != got.is_nan() || (want - got).abs() > scale * 1e-3 {
                return format!("{name} at step {step}: Vensim {want}, Simlin {got}");
            }
        }
    }
    "a series differs within the comparison's tolerance".to_string()
}

/// Every file that does not match Vensim, by how and why, with spreadsheets
/// readable (the `ext_data` feature). Found by running the gate. A cause is
/// what the first difference or the first errors show; one that was not run
/// down says so.
const EXPECTED: &[(&str, Class, &str)] = &[
    (
        "test/sdeverywhere/models/delayfixed/delayfixed.mdl",
        Class::DoesNotSimulate,
        "DELAY FIXED delays its input by exactly the delay time, which the engine refuses rather than approximate",
    ),
    (
        "test/sdeverywhere/models/delayfixed2/delayfixed2.mdl",
        Class::DoesNotSimulate,
        "DELAY FIXED, as delayfixed",
    ),
    (
        "test/sdeverywhere/models/directdata/directdata.mdl",
        Class::Differs,
        "a GET DIRECT DATA series reads another cell range than Vensim's; cause not established",
    ),
    (
        "test/sdeverywhere/models/extdata/extdata.mdl",
        Class::Differs,
        "its data variables come from extdata_data.dat, which the reader does not load",
    ),
    (
        "test/sdeverywhere/models/flatten/expected.mdl",
        Class::Differs,
        "expected.dat names its quoted variables another way; the pair is a fixture of the flatten tool, not a model run",
    ),
    (
        "test/sdeverywhere/models/getdata/getdata.mdl",
        Class::DoesNotSimulate,
        "GET DATA AT TIME and its relatives are not read",
    ),
    (
        "test/sdeverywhere/models/prune/prune.mdl",
        Class::Differs,
        "its data variables come from prune_data.dat, which the reader does not load",
    ),
    (
        "test/sdeverywhere/models/sumif/sumif.mdl",
        Class::Differs,
        "its data variables come from sumif_data.dat, which the reader does not load",
    ),
    (
        "test/test-models/samples/Query_file/Query_file.mdl",
        Class::DoesNotSimulate,
        "RANDOM UNIFORM is read as a function the engine does not have",
    ),
    (
        "test/test-models/samples/Roessler_Chaos/roessler_chaos.mdl",
        Class::Differs,
        "a chaotic model: the runs part after two thousand steps (unverified that rounding alone is the cause)",
    ),
    (
        "test/test-models/tests/allocate_available/test_allocate_available.mdl",
        Class::Differs,
        "ALLOCATE AVAILABLE gives another allocation from step 1; cause not established",
    ),
    (
        "test/test-models/tests/allocate_by_priority/test_allocate_by_priority.mdl",
        Class::Differs,
        "ALLOCATE BY PRIORITY gives another allocation over two dimensions; cause not established",
    ),
    (
        "test/test-models/tests/arguments/test_arguments.mdl",
        Class::DoesNotSimulate,
        "SMOOTH N of order 2",
    ),
    (
        "test/test-models/tests/conditional_subscripts/test_conditional_subscripts.mdl",
        Class::Differs,
        "a subrange's name as a value is the element's position in the subrange, where Vensim's is its position in the dimension that owns it (dimA = subA holds on the diagonal)",
    ),
    (
        "test/test-models/tests/control_vars/test_control_vars.mdl",
        Class::Differs,
        "its control variables are not constants, so it runs with the defaults",
    ),
    (
        "test/test-models/tests/delay_fixed/test_delay_fixed.mdl",
        Class::DoesNotSimulate,
        "DELAY FIXED, as delayfixed",
    ),
    (
        "test/test-models/tests/delay_numeric_error/test_delay_numeric_error.mdl",
        Class::DoesNotSimulate,
        "DELAY N of order 6",
    ),
    (
        "test/test-models/tests/delay_pipeline/test_pipeline_delays.mdl",
        Class::DoesNotSimulate,
        "DELAY N of order 2",
    ),
    (
        "test/test-models/tests/delays/test_delays.mdl",
        Class::DoesNotSimulate,
        "DELAY N whose order is a variable",
    ),
    (
        "test/test-models/tests/dynamic_final_time/test_dynamic_final_time.mdl",
        Class::Differs,
        "FINAL TIME is not a constant, so it runs with the default",
    ),
    (
        "test/test-models/tests/forecast/test_forecast.mdl",
        Class::DoesNotSimulate,
        "FORECAST is read as a function the engine does not have",
    ),
    (
        "test/test-models/tests/game/test_game.mdl",
        Class::DoesNotSimulate,
        "GAME is not read",
    ),
    (
        "test/test-models/tests/get_constants_incomplete_subscript/test_get_constants_incomplete_subscript.mdl",
        Class::DoesNotImport,
        "GET XLS CONSTANTS over a range holding a cell that is not a number",
    ),
    (
        "test/test-models/tests/get_constants_subranges/test_get_constants_subranges.mdl",
        Class::DoesNotImport,
        "GET DIRECT CONSTANTS reads past the sheet; cause not established",
    ),
    (
        "test/test-models/tests/get_lookups_subscripted_args/test_get_lookups_subscripted_args.mdl",
        Class::DoesNotImport,
        "GET XLS LOOKUPS as a lookup's definition does not parse",
    ),
    (
        "test/test-models/tests/get_lookups_subset/test_get_lookups_subset.mdl",
        Class::DoesNotImport,
        "GET XLS LOOKUPS as a lookup's definition does not parse",
    ),
    (
        "test/test-models/tests/get_mixed_definitions/test_get_mixed_definitions.mdl",
        Class::DoesNotImport,
        "a named cell range is read as a column",
    ),
    (
        "test/test-models/tests/get_values_order/test_get_values_order.mdl",
        Class::DoesNotImport,
        "a named cell range is read as a cell",
    ),
    (
        "test/test-models/tests/get_with_missing_values_xlsx/test_get_with_missing_values_xlsx.mdl",
        Class::DoesNotSimulate,
        "GET XLS DATA leaves its variables without equations",
    ),
    (
        "test/test-models/tests/get_xls_cellrange/test_get_xls_cellrange.mdl",
        Class::DoesNotImport,
        "a named cell range is read as a cell",
    ),
    (
        "test/test-models/tests/input_functions/test_inputs.mdl",
        Class::Differs,
        "output.tab holds a one-step pulse every 2 from 3, which is the XMILE twin's PULSE and not this file's PULSE(3, 2)",
    ),
    (
        "test/test-models/tests/invert_matrix/test_invert_matrix.mdl",
        Class::DoesNotSimulate,
        "INVERT MATRIX is not read",
    ),
    (
        "test/test-models/tests/na/test_na.mdl",
        Class::Differs,
        "output.tab leaves :NA: cells empty, which the comparison reads as the value before",
    ),
    (
        "test/test-models/tests/odd_number_quotes/teacup_3quotes.mdl",
        Class::DoesNotImport,
        "a units range bound written inf does not parse",
    ),
    (
        "test/test-models/tests/power/power.mdl",
        Class::DoesNotSimulate,
        "POWER is read as a function the engine does not have",
    ),
    (
        "test/test-models/tests/reality_checks/test_reality_checks.mdl",
        Class::DoesNotImport,
        "reality check equations (:THE CONDITION:) do not parse",
    ),
    (
        "test/test-models/tests/smooth/test_smooth.mdl",
        Class::DoesNotSimulate,
        "SMOOTH N whose order is a variable",
    ),
    (
        "test/test-models/tests/special_characters/test_special_variable_names.mdl",
        Class::DoesNotSimulate,
        "a reference to a name holding escaped quotes and a backslash is written as text the equation lexer does not read",
    ),
    (
        "test/test-models/tests/subscript_aggregation/test_subscript_aggregation.mdl",
        Class::DoesNotSimulate,
        "PROD is read as a function the engine does not have",
    ),
    (
        "test/test-models/tests/subscript_definition/test_subscript_definition.mdl",
        Class::Differs,
        "a subrange's name as a value, as conditional_subscripts",
    ),
    (
        "test/test-models/tests/subscript_logicals/test_subscript_logicals.mdl",
        Class::Differs,
        "a dimension's name as a value is the element's position in it, where Vensim's is its position in the dimension that owns the element (dim1 = dim3 over the same elements in another order)",
    ),
    (
        "test/test-models/tests/subscript_switching/subscript_switching.mdl",
        Class::DoesNotSimulate,
        "a reference whose dimensions the engine does not match to its equation's",
    ),
    (
        "test/test-models/tests/subscripted_delays/test_subscripted_delays.mdl",
        Class::DoesNotSimulate,
        "DELAY N of order 6",
    ),
    (
        "test/test-models/tests/subscripted_logicals/test_subscripted_logicals.mdl",
        Class::Differs,
        "the precedence of :NOT:, :AND: and :OR: (GH #914)",
    ),
    (
        "test/test-models/tests/subscripted_smooth/test_subscripted_smooth.mdl",
        Class::DoesNotSimulate,
        "SMOOTH N whose order is a variable",
    ),
    (
        "test/test-models/tests/subscripted_trig/test_subscripted_trig.mdl",
        Class::DoesNotSimulate,
        "COSH, SINH and TANH are read as variables",
    ),
    (
        "test/test-models/tests/vector_order/test_vector_order.mdl",
        Class::DoesNotImport,
        "a named cell range is read as a column",
    ),
    (
        "test/test-models/tests/vector_select/test_vector_select.mdl",
        Class::Differs,
        "VECTOR SELECT gives another value; cause not established",
    ),
    (
        "test/test-models/tests/with_lookup/test_with_lookup.mdl",
        Class::DoesNotImport,
        "a named cell range is read as a cell",
    ),
];

/// The files that read a spreadsheet. Without the `ext_data` feature the
/// reader refuses each of them, whatever `EXPECTED` says of it.
const READS_A_SPREADSHEET: &[&str] = &[
    "test/sdeverywhere/models/directdata/directdata.mdl",
    "test/test-models/tests/get_constants_incomplete_subscript/test_get_constants_incomplete_subscript.mdl",
    "test/test-models/tests/get_constants_subranges/test_get_constants_subranges.mdl",
    "test/test-models/tests/get_mixed_definitions/test_get_mixed_definitions.mdl",
    "test/test-models/tests/get_subscript_3d_arrays_xls/test_get_subscript_3d_arrays_xls.mdl",
    "test/test-models/tests/get_values_order/test_get_values_order.mdl",
    "test/test-models/tests/get_xls_cellrange/test_get_xls_cellrange.mdl",
    "test/test-models/tests/partial_range_definitions/test_partial_range_definitions.mdl",
    "test/test-models/tests/vector_order/test_vector_order.mdl",
    "test/test-models/tests/with_lookup/test_with_lookup.mdl",
];

/// The class `path` is listed in under the features this build has.
fn listed_class(path: &str) -> Class {
    if !cfg!(feature = "ext_data") && READS_A_SPREADSHEET.contains(&path) {
        return Class::DoesNotImport;
    }
    EXPECTED
        .iter()
        .find(|(listed, _, _)| *listed == path)
        .map_or(Class::Matches, |(_, class, _)| *class)
}

/// One small corpus file per class, so the default suite holds the
/// comparison itself: a classifier that called everything a match, or
/// nothing, would pass the corpus gate's bookkeeping only until it ran.
#[test]
fn each_class_is_told_from_the_others() {
    let rows = [
        (
            Class::Matches,
            "test/test-models/tests/rounding/test_rounding.mdl",
        ),
        (Class::Differs, "test/test-models/tests/na/test_na.mdl"),
        (
            Class::DoesNotSimulate,
            "test/test-models/tests/power/power.mdl",
        ),
        (
            Class::DoesNotImport,
            "test/test-models/tests/odd_number_quotes/teacup_3quotes.mdl",
        ),
    ];
    let mut classes: Vec<Class> = rows.iter().map(|(class, _)| *class).collect();
    classes.sort();
    assert_eq!(classes, Class::ALL, "a class has no row");
    for (class, path) in rows {
        assert_eq!(listed_class(path), class, "{path} is listed otherwise");
        let file = PathBuf::from("../..").join(path);
        let output = vensim_output(&file, true)
            .unwrap_or_else(|| panic!("{path} has no Vensim output beside it"));
        let (found, detail, _) = classify(path, &output);
        assert_eq!(found, class, "{path}: {detail}");
    }
}

#[test]
#[ignore = "imports and simulates every corpus .mdl with Vensim output; run under the gates profile"]
fn the_mdl_reader_matches_vensim_over_the_corpus() {
    let corpus = corpus();
    assert!(
        corpus.len() >= MIN_COMPARED,
        "only {} corpus models have Vensim output",
        corpus.len()
    );
    let listed_once: BTreeMap<&str, Class> = EXPECTED
        .iter()
        .map(|(path, class, _)| (*path, *class))
        .collect();
    assert_eq!(listed_once.len(), EXPECTED.len(), "a file is listed twice");
    let mut tally: BTreeMap<Class, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut listed_series = differing_series();
    let mut found_series: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (path, output) in &corpus {
        let (found, detail, differ) = classify(path, output);
        *tally.entry(found).or_default() += 1;
        let listed = listed_class(path);
        if found != listed {
            failures.push(format!(
                "(\"{path}\", Class::{found:?}, \"\"), // listed {listed:?}: {detail}"
            ));
        }
        if found == Class::Differs {
            let want = listed_series
                .get(path.as_str())
                .cloned()
                .unwrap_or_default();
            for series in differ.iter().filter(|s| !want.contains(s)) {
                failures.push(format!("{path}: {series} differs and is not listed"));
            }
            for series in want.iter().filter(|s| !differ.contains(s)) {
                failures.push(format!("{path}: {series} is listed and matches"));
            }
            found_series.insert(path.clone(), differ);
        }
    }
    for path in listed_series.keys() {
        if EXPECTED
            .iter()
            .all(|(listed, class, _)| listed != path || *class != Class::Differs)
        {
            failures.push(format!(
                "{path} has series listed and is not listed as differing"
            ));
        }
    }
    if std::env::var_os("UPDATE_VENSIM_TRUTH_SERIES").is_some() {
        // A file this build does not run to a difference (a spreadsheet the
        // build cannot read) keeps its rows.
        for (path, series) in found_series {
            listed_series.insert(path, series);
        }
        let mut table = String::new();
        for (path, series) in &listed_series {
            let mut series = series.clone();
            series.sort();
            for one in series {
                table.push_str(&format!("{path}\t{one}\n"));
            }
        }
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/integration/mdl_vensim_truth_series.tsv");
        fs::write(file, table).expect("the table writes");
        return;
    }
    let listed_paths = EXPECTED
        .iter()
        .map(|(path, _, _)| path)
        .chain(READS_A_SPREADSHEET);
    for path in listed_paths {
        if !corpus.iter().any(|(p, _)| p == path) {
            failures.push(format!("{path} is listed and was not compared"));
        }
    }
    eprintln!("{tally:?}");
    assert!(
        failures.is_empty(),
        "{} files are not in their listed class:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
