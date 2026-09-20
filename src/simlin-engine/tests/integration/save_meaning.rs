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
//! a format can hold less than another, and a host asks before it saves
//! across formats.

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
    let (a, b) = (columns(original), columns(save));
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
