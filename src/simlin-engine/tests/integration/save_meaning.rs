// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A save keeps what the model means: for every corpus model that simulates,
//! the model a save reads back as simulates exactly as the model did, column
//! for column and value for value (-0 is 0, and NaN is NaN).
//!
//! Each model is saved in its own format (MDL for `.mdl`, XMILE for XMILE and
//! Stella files), in native JSON and in protobuf. The saves this gate finds
//! changing a model today are listed, by what goes wrong, in
//! `EXPECTED_CHANGES`. It is a ratchet: a save that starts changing a model
//! fails the gate, and so does a listed save that stops, so a fix removes the
//! entries it repairs.
//!
//! A save in another format (an XMILE model saved as MDL) is not gated here:
//! a format can hold less than another, and a host asks `check_save`
//! (`simlin_engine::save_check`) before it saves across formats. The second
//! test holds that check to what the gate finds, across formats too.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::BufReader;
use std::path::PathBuf;

use simlin_engine::buffa::Message;
use simlin_engine::datamodel::Project;
use simlin_engine::db::{
    LtmOverlay, SimlinDb, compile_project_incremental, sync_from_datamodel_incremental,
};
use simlin_engine::{Results, Vm};

/// Files larger than this are left out, so a debug build stays within its
/// time budget, as the MDL writer's corpus ratchets do; only C-LEARN is left
/// out today.
const MAX_BYTES: u64 = 200 * 1024;

/// Fewer models than this reaching a comparison means the corpus walk or the
/// readers broke, which would otherwise pass the gate vacuously.
const MIN_COMPARED: usize = 400;

/// The formats a model is saved in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Format {
    Mdl,
    Xmile,
    Json,
    Protobuf,
}

/// Every (file, format) whose save changes what the model simulates today,
/// by what the save does wrong. Found by running this gate.
const EXPECTED_CHANGES: &[(&str, Format)] = &[
    // MDL writes an arrayed variable's elements one equation each, and an
    // element-only definition reads back over the elements' whole family:
    // a variable over a subrange (SubA of DimA), or over one dimension of
    // several that share its elements, comes back over the other one. It
    // gains elements, or no longer fits the equations that use it.
    (
        "test/sdeverywhere/models/arrays_cname/arrays_cname.mdl",
        Format::Mdl,
    ),
    (
        "test/sdeverywhere/models/arrays_varname/arrays_varname.mdl",
        Format::Mdl,
    ),
    ("test/sdeverywhere/models/delay/delay.mdl", Format::Mdl),
    ("test/sdeverywhere/models/except/except.mdl", Format::Mdl),
    ("test/sdeverywhere/models/except2/except2.mdl", Format::Mdl),
    ("test/sdeverywhere/models/smooth/smooth.mdl", Format::Mdl),
    ("test/sdeverywhere/models/sum/sum.mdl", Format::Mdl),
    (
        "test/test-models/tests/allocate_by_priority/test_allocate_by_priority.mdl",
        Format::Mdl,
    ),
    (
        "test/test-models/tests/data_from_other_model/test_data_from_other_model.mdl",
        Format::Mdl,
    ),
    (
        "test/test-models/tests/subscripted_ramp_step/test_subscripted_ramp_step.mdl",
        Format::Mdl,
    ),
    // MDL drops an :EXCEPT: default that names the variable's own
    // dimensions, so the elements only the default defined read back as 0.
    (
        "test/test-models/tests/except_subranges/test_except_subranges.mdl",
        Format::Mdl,
    ),
];

/// Every model file under `test/` no larger than `MAX_BYTES`, sorted, by
/// its path from the checkout's root.
fn corpus() -> Vec<String> {
    let root = PathBuf::from("../..");
    let mut files = Vec::new();
    let mut dirs = vec![root.join("test")];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            let model = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                ["mdl", "xmile", "stmx", "itmx"]
                    .iter()
                    .any(|x| e.eq_ignore_ascii_case(x))
            });
            if path.is_dir() {
                dirs.push(path);
            } else if model && fs::metadata(&path).is_ok_and(|m| m.len() <= MAX_BYTES) {
                let rel = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                files.push(rel);
            }
        }
    }
    files.sort();
    files
}

fn is_mdl(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".mdl")
}

fn open(path: &str) -> Option<Project> {
    let bytes = fs::read(format!("../../{path}")).ok()?;
    if is_mdl(path) {
        simlin_engine::open_vensim(std::str::from_utf8(&bytes).ok()?).ok()
    } else {
        simlin_engine::open_xmile(&mut BufReader::new(bytes.as_slice())).ok()
    }
}

fn simulate(project: &Project) -> Result<Results, String> {
    let main = if project.models.iter().any(|m| m.name == "main") {
        "main".to_string()
    } else {
        project
            .models
            .first()
            .map(|m| m.name.clone())
            .unwrap_or_default()
    };
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    let compiled = compile_project_incremental(&db, sync.project, &main, LtmOverlay::Off)
        .map_err(|e| e.to_string())?;
    let mut vm = Vm::new(compiled).map_err(|e| e.to_string())?;
    vm.run_to_end().map_err(|e| e.to_string())?;
    Ok(vm.into_results())
}

/// The model a save of `project` in `format` reads back as.
fn saved(project: &Project, format: Format) -> Result<Project, String> {
    match format {
        Format::Mdl => {
            let text = simlin_engine::to_mdl(project).map_err(|e| format!("writing: {e}"))?;
            simlin_engine::open_vensim(&text).map_err(|e| format!("reading: {e}"))
        }
        Format::Xmile => {
            let text = simlin_engine::to_xmile(project).map_err(|e| format!("writing: {e}"))?;
            simlin_engine::open_xmile(&mut BufReader::new(text.as_bytes()))
                .map_err(|e| format!("reading: {e}"))
        }
        Format::Json => {
            let json: simlin_engine::json::Project = project.clone().into();
            let bytes = serde_json::to_vec(&json).map_err(|e| format!("writing: {e}"))?;
            let back = simlin_engine::json::Project::from_reader(bytes.as_slice())
                .map_err(|e| format!("reading: {e}"))?;
            Ok(back.into())
        }
        Format::Protobuf => {
            let pb =
                simlin_engine::serde::serialize(project).map_err(|e| format!("writing: {e:?}"))?;
            let bytes = pb
                .try_encode_to_vec()
                .map_err(|e| format!("writing: {e:?}"))?;
            let back = simlin_engine::project_io::Project::decode_from_slice(&bytes)
                .map_err(|e| format!("reading: {e:?}"))?;
            Ok(simlin_engine::serde::deserialize(back))
        }
    }
}

/// Each column of `results`, by name.
fn columns(results: &Results) -> BTreeMap<String, Vec<u64>> {
    results
        .offsets
        .iter()
        .map(|(name, &at)| {
            let column = results
                .iter()
                .map(|row| {
                    // Every NaN is the same value here, and -0 is 0.
                    if row[at].is_nan() {
                        f64::NAN.to_bits()
                    } else if row[at] == 0.0 {
                        0.0f64.to_bits()
                    } else {
                        row[at].to_bits()
                    }
                })
                .collect();
            (name.to_string(), column)
        })
        .collect()
}

/// How the save's results differ from the original's; None when they are the
/// same, value for value, in every column.
fn difference(original: &Results, save: &Results) -> Option<String> {
    difference_leaving_out(original, save, &[])
}

/// [`difference`], leaving out the columns named in `left_out`.
fn difference_leaving_out(original: &Results, save: &Results, left_out: &[&str]) -> Option<String> {
    let (mut a, mut b) = (columns(original), columns(save));
    for name in left_out {
        a.remove(*name);
        b.remove(*name);
    }
    let lost: Vec<&String> = a.keys().filter(|k| !b.contains_key(*k)).collect();
    let gained: Vec<&String> = b.keys().filter(|k| !a.contains_key(*k)).collect();
    let changed: Vec<&String> = a
        .iter()
        .filter(|(k, v)| b.get(*k).is_some_and(|w| w != *v))
        .map(|(k, _)| k)
        .collect();
    if lost.is_empty() && gained.is_empty() && changed.is_empty() {
        return None;
    }
    let first = |names: &[&String]| names.first().map(|n| n.as_str()).unwrap_or("").to_string();
    Some(format!(
        "{} columns change (first {}), {} are lost (first {}), {} are gained (first {})",
        changed.len(),
        first(&changed),
        lost.len(),
        first(&lost),
        gained.len(),
        first(&gained)
    ))
}

/// Why the save of `path` in `format` changes what the model simulates, or
/// None when it does not.
fn outcome(original: &Project, results: &Results, format: Format) -> Option<String> {
    match saved(original, format) {
        Err(why) => Some(why),
        Ok(save) => match simulate(&save) {
            Err(why) => Some(format!("the save does not simulate: {why}")),
            Ok(save_results) => difference(results, &save_results),
        },
    }
}

#[test]
fn a_save_keeps_what_the_model_simulates() {
    use rayon::prelude::*;

    let checks: Vec<(String, Format, Option<String>)> = corpus()
        .into_par_iter()
        .flat_map_iter(|path| {
            let Some(project) = open(&path) else {
                return Vec::new();
            };
            let Ok(results) = simulate(&project) else {
                return Vec::new();
            };
            let formats: &[Format] = if is_mdl(&path) {
                &[Format::Mdl, Format::Json, Format::Protobuf]
            } else {
                &[Format::Xmile, Format::Json, Format::Protobuf]
            };
            formats
                .iter()
                .map(|&format| (path.clone(), format, outcome(&project, &results, format)))
                .collect()
        })
        .collect();

    assert!(
        checks.len() >= MIN_COMPARED,
        "only {} saves were compared",
        checks.len()
    );
    let expected: BTreeSet<(&str, Format)> = EXPECTED_CHANGES.iter().copied().collect();
    let mut failures = Vec::new();
    for (path, format, change) in &checks {
        match (change, expected.contains(&(path.as_str(), *format))) {
            (Some(why), false) => {
                failures.push(format!("{path} as {format:?} changes the model: {why}"))
            }
            (None, true) => failures.push(format!(
                "{path} as {format:?} keeps the model now; remove it from EXPECTED_CHANGES"
            )),
            _ => {}
        }
    }
    let checked: BTreeSet<(&str, Format)> =
        checks.iter().map(|(p, f, _)| (p.as_str(), *f)).collect();
    for entry in &expected {
        if !checked.contains(entry) {
            failures.push(format!(
                "{} as {:?} is listed but was not checked",
                entry.0, entry.1
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The saves `check_save` reports changing though the gate's run shows no
/// difference: what only the structure shows, found by running this test.
const EXPECTED_STRUCTURE_ONLY: &[(&str, Format)] = &[
    // MDL cannot mark a variable non-negative, and these runs never go
    // below zero.
    (
        "test/ai-information/GeneratedByAIThenEdited.stmx",
        Format::Mdl,
    ),
    ("test/test-models/samples/teacup/teacup.stmx", Format::Mdl),
    (
        "test/test-models/tests/delay_xmile/test_delay_xmile.xmile",
        Format::Mdl,
    ),
    (
        "test/test-models/tests/non_negative_flows/test_non_negative_flows.xmile",
        Format::Mdl,
    ),
    (
        "test/test-models/tests/non_negative_flows/test_non_negative_flows_behavior.xmile",
        Format::Mdl,
    ),
    // MDL drops an :EXCEPT: default the compiler applies where every
    // element is written out, so no element takes it today, though one a
    // dimension gained would.
    ("test/test-models/tests/except/test_except.mdl", Format::Mdl),
    (
        "test/test-models/tests/except_multiple/test_except_multiple.mdl",
        Format::Mdl,
    ),
    // A macro's input with no equation is written back as `0`; the macro
    // is always called with it bound.
    (
        "test/test-models/tests/macro_stock/test_macro_stock.xmile",
        Format::Xmile,
    ),
    (
        "test/test-models/tests/macro_stock/test_macro_stock.xmile",
        Format::Mdl,
    ),
    // A variable reads back over another dimension of the same elements,
    // which these runs cannot tell apart (the element-only definitions
    // `EXPECTED_CHANGES` lists first).
    (
        "test/sdeverywhere/models/subalias/subalias.mdl",
        Format::Mdl,
    ),
    (
        "test/test-models/tests/subscript_copy/test_subscript_copy.mdl",
        Format::Mdl,
    ),
    (
        "test/test-models/tests/subscript_copy/test_subscript_copy2.mdl",
        Format::Mdl,
    ),
    (
        "test/test-models/tests/subscript_subranges_equal/test_subscript_subrange_equal.mdl",
        Format::Mdl,
    ),
    (
        "test/test-models/tests/subscript_transposition/test_subscript_transposition.mdl",
        Format::Mdl,
    ),
    (
        "test/test-models/tests/subset_duplicated_coord/test_subset_duplicated_coord.mdl",
        Format::Mdl,
    ),
];

/// The control variables a model can hold as variables, which a format that
/// keeps them as its specs reads back as specs: the check calls that no
/// change when the values agree, so their columns are left out here.
const CONTROL_COLUMNS: &[&str] = &["initial_time", "final_time", "time_step", "saveper"];

/// `check_save` holds to what this gate finds, in the formats the gate saves
/// in and as MDL for an XMILE model (the Save As a host would refuse): a
/// save the gate finds changing a model has a change to its results, a save
/// the gate finds keeping it has none, and a change to its definition only
/// where the structure shows what the run does not (`EXPECTED_STRUCTURE_ONLY`).
#[test]
fn the_check_names_every_save_the_gate_finds_changing() {
    use rayon::prelude::*;
    use simlin_engine::save_check::{ChangeKind, SaveFormat, check_save};

    let check_format = |format: Format| match format {
        Format::Mdl => SaveFormat::Mdl,
        Format::Xmile => SaveFormat::Xmile,
        Format::Json => SaveFormat::Json,
        Format::Protobuf => SaveFormat::Protobuf,
    };
    // Each save's gate verdict, and the check's changes to results and to
    // the definition alone.
    type Checked = (String, Format, bool, Result<(usize, usize), String>);
    let results: Vec<Checked> = corpus()
        .into_par_iter()
        .flat_map_iter(|path| {
            let Some(project) = open(&path) else {
                return Vec::new();
            };
            let Ok(results) = simulate(&project) else {
                return Vec::new();
            };
            let formats: &[Format] = if is_mdl(&path) {
                &[Format::Mdl, Format::Json, Format::Protobuf]
            } else {
                &[Format::Xmile, Format::Json, Format::Protobuf, Format::Mdl]
            };
            formats
                .iter()
                .filter_map(|&format| {
                    let gate_change = match saved(&project, format) {
                        // A format that cannot hold the model: the check refuses it too.
                        Err(why) if why.starts_with("writing") => return None,
                        Err(_) => true,
                        Ok(save) => match simulate(&save) {
                            Err(_) => true,
                            Ok(save_results) => {
                                difference_leaving_out(&results, &save_results, CONTROL_COLUMNS)
                                    .is_some()
                            }
                        },
                    };
                    let check = check_save(&project, check_format(format))
                        .map(|changes| {
                            let results = changes
                                .iter()
                                .filter(|c| c.kind == ChangeKind::Results)
                                .count();
                            (results, changes.len() - results)
                        })
                        .map_err(|e| e.to_string());
                    Some((path.clone(), format, gate_change, check))
                })
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        results.len() >= MIN_COMPARED,
        "only {} saves were checked",
        results.len()
    );
    let structure_only: BTreeSet<(&str, Format)> =
        EXPECTED_STRUCTURE_ONLY.iter().copied().collect();
    let mut failures = Vec::new();
    for (path, format, gate_change, check) in &results {
        let listed = structure_only.contains(&(path.as_str(), *format));
        match (gate_change, check) {
            (_, Err(why)) => failures.push(format!("{path} as {format:?}: the check refused: {why}")),
            (true, Ok((0, _))) => failures.push(format!(
                "{path} as {format:?} changes the model's results, and the check says they do not change"
            )),
            (false, Ok((n, _))) if *n > 0 => failures.push(format!(
                "{path} as {format:?}: the check reports {n} changes to results the run does not show"
            )),
            (false, Ok((0, n))) if *n > 0 && !listed => failures.push(format!(
                "{path} as {format:?}: the check reports {n} changes to the definition the run does not show"
            )),
            (false, Ok((0, 0))) if listed => failures.push(format!(
                "{path} as {format:?}: the check reports none now; remove it from EXPECTED_STRUCTURE_ONLY"
            )),
            _ => {}
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A model that reads external data is read back with the data provider it
/// was opened with, so its save's references resolve as the model's did.
/// Without it, the save does not read back.
#[test]
fn a_save_that_reads_data_is_read_back_with_its_provider() {
    use simlin_engine::save_check::{ChangeKind, SaveFormat, check_save, check_save_with_data};
    let path = "../../test/test-models/tests/get_data/test_get_data.mdl";
    let dir = std::path::Path::new(path).parent().unwrap();
    let provider = simlin_engine::FilesystemDataProvider::new(dir);
    let text = fs::read_to_string(path).unwrap();
    let project = simlin_engine::open_vensim_with_data(&text, Some(&provider)).unwrap();

    let changes = check_save_with_data(&project, SaveFormat::Mdl, Some(&provider)).unwrap();
    assert!(changes.is_empty(), "{changes:?}");

    let changes = check_save(&project, SaveFormat::Mdl).unwrap();
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert!(
        changes[0]
            .reason
            .starts_with("the save does not read back: "),
        "{changes:?}"
    );
    assert_eq!(changes[0].kind, ChangeKind::Results);
}
