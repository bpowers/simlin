// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Everything the engine hands a host is a function of the file.
//!
//! A model file is opened twice in one process and every artifact is built
//! from each open. Two `HashMap`s of the same contents iterate in different
//! orders within a process (each takes its own `RandomState`), so an artifact
//! that differs between the two builds took its order from a map: the class
//! the salsa layer's "never let `HashMap` iteration order reach a cached value
//! or the compiled artifact" rule is about, held here on the surfaces a host
//! reads rather than on one query.
//!
//! The artifacts are the import warnings, the protobuf, JSON, XMILE and MDL
//! saves, the diagnostics as hosts format them, the causal links (collapsed
//! and raw), the wasm blob and its layout, the results as TSV, and the loop
//! analysis. One listed in [`artifacts`] is covered; a surface added to the
//! engine is not until it is listed there.

use std::collections::BTreeMap;
use std::fs;
use std::io::BufReader;
use std::path::PathBuf;

use simlin_engine::buffa::Message;
use simlin_engine::datamodel::Project;
use simlin_engine::db::{
    LtmOverlay, SimlinDb, collect_all_diagnostics, compile_project_incremental,
    sync_from_datamodel_incremental,
};

/// Loop analysis compiles the model under the LTM overlay and searches its
/// loops, so only files up to this size take it.
const LOOP_ANALYSIS_MAX_BYTES: usize = 20 * 1024;

/// Every model file under `test/`, relative to the repo root.
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
            } else if model && let Ok(relative) = path.strip_prefix(&root) {
                files.push(relative.to_string_lossy().into_owned());
            }
        }
    }
    files.sort();
    files
}

fn open(path: &str, bytes: &[u8]) -> Option<(Project, Vec<String>)> {
    let (project, warnings) = if path.to_ascii_lowercase().ends_with(".mdl") {
        simlin_engine::open_vensim_with_warnings(std::str::from_utf8(bytes).ok()?).ok()?
    } else {
        simlin_engine::open_xmile_with_warnings(&mut BufReader::new(bytes)).ok()?
    };
    Some((project, warnings.into_iter().map(|w| w.message).collect()))
}

/// Every artifact built from one open of a file, each as bytes, by name.
/// `None` for a file that does not open.
fn artifacts(path: &str, bytes: &[u8]) -> Option<BTreeMap<&'static str, Vec<u8>>> {
    let (project, warnings) = open(path, bytes)?;
    let mut out: BTreeMap<&'static str, Vec<u8>> = BTreeMap::new();
    out.insert("import warnings", warnings.join("\n").into_bytes());
    if let Ok(stored) = simlin_engine::serde::serialize(&project) {
        out.insert("protobuf", stored.encode_to_vec());
    }
    let json: simlin_engine::json::Project = project.clone().into();
    if let Ok(json) = serde_json::to_vec(&json) {
        out.insert("json", json);
    }
    if let Ok(text) = simlin_engine::to_xmile(&project) {
        out.insert("xmile", text.into_bytes());
    }
    if let Ok((text, warnings)) = simlin_engine::compat::to_mdl_with_warnings(&project) {
        out.insert("mdl", text.into_bytes());
        out.insert(
            "mdl export warnings",
            warnings
                .iter()
                .map(|w| format!("{w:?}"))
                .collect::<Vec<_>>()
                .join("\n")
                .into_bytes(),
        );
    }

    let main = if project.models.iter().any(|m| m.name == "main") {
        "main".to_string()
    } else {
        project.models.first().map(|m| m.name.clone())?
    };
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);

    let diagnostics = collect_all_diagnostics(&db, sync.project, LtmOverlay::Off);
    let formatted = simlin_engine::errors::collect_formatted_errors(&diagnostics, &project);
    out.insert(
        "diagnostics",
        formatted
            .errors
            .iter()
            .map(|e| format!("{e:?}"))
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes(),
    );

    // The links before compiling, as a host's structural query reads them. A
    // module cycle is refused before any recursive query runs.
    if simlin_engine::db::project_module_graph(&db, sync.project)
        .cycle_error_from(&main)
        .is_none()
        && let Some(model) = sync
            .project
            .models(&db)
            .get(simlin_engine::canonicalize(&main).as_ref())
    {
        for (name, internal) in [("links", false), ("links (internal)", true)] {
            let links =
                simlin_engine::analysis::model_links(&db, *model, sync.project, None, internal);
            out.insert(
                name,
                links
                    .iter()
                    .map(|l| format!("{} -> {} {:?}", l.from, l.to, l.polarity))
                    .collect::<Vec<_>>()
                    .join("\n")
                    .into_bytes(),
            );
        }
    }

    if let Ok(compiled) = compile_project_incremental(&db, sync.project, &main, LtmOverlay::Off) {
        if let Ok(artifact) = simlin_engine::wasmgen::compile_simulation(&compiled) {
            out.insert("wasm blob", artifact.wasm);
            out.insert("wasm layout", artifact.layout.serialize());
        }
        if let Ok(mut vm) = simlin_engine::Vm::new(compiled)
            && vm.run_to_end().is_ok()
        {
            let mut tsv = Vec::new();
            if vm.into_results().write_tsv(&mut tsv, None).is_ok() {
                out.insert("results tsv", tsv);
            }
        }
    }

    if bytes.len() <= LOOP_ANALYSIS_MAX_BYTES {
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        if let Ok(analysis) =
            simlin_engine::analysis::analyze_model(&project, &mut db, sync.project, &main, None)
        {
            let mut text = String::new();
            for l in &analysis.loop_dominance {
                text.push_str(&format!(
                    "{} {:?} {} {:?} {:?} {:?}\n",
                    l.loop_id,
                    l.name,
                    l.polarity,
                    l.variables,
                    l.partition,
                    l.importance.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
                ));
            }
            for p in &analysis.dominant_loops_by_period {
                text.push_str(&format!(
                    "{} {} {:?} {}\n",
                    p.start,
                    p.end,
                    p.dominant_loops,
                    p.combined_score.to_bits()
                ));
            }
            text.push_str(&format!("{:?}", analysis.analysis_error));
            out.insert("loop analysis", text.into_bytes());
        }
    }
    Some(out)
}

/// The artifacts that differ between two builds of `path`; `None` for a file
/// that does not open.
fn differing_artifacts(path: &str) -> Option<Vec<&'static str>> {
    let bytes = fs::read(format!("../../{path}")).ok()?;
    let first = artifacts(path, &bytes)?;
    let second = artifacts(path, &bytes)?;
    let mut names: Vec<&'static str> = first
        .iter()
        .filter(|(name, bytes)| second.get(*name) != Some(*bytes))
        .map(|(name, _)| *name)
        .collect();
    names.extend(second.keys().filter(|name| !first.contains_key(*name)));
    Some(names)
}

/// A scalar model, a model of modules and an arrayed one, each with enough
/// variables that two maps of them iterate in different orders.
#[test]
fn every_artifact_is_a_function_of_the_file() {
    for path in [
        "test/test-models/samples/SIR/SIR.xmile",
        "test/modules_hares_and_foxes/modules_hares_and_foxes.stmx",
        "test/sdeverywhere/models/subscript/subscript.mdl",
    ] {
        let differing = differing_artifacts(path).expect("the corpus model opens");
        assert!(
            differing.is_empty(),
            "{path}: two builds differ in {differing:?}"
        );
    }
}

#[test]
#[ignore = "every corpus model built twice; run under the gates profile"]
fn every_artifact_of_every_corpus_model_is_a_function_of_the_file() {
    use rayon::prelude::*;

    let built: Vec<(String, Vec<&'static str>)> = corpus()
        .into_par_iter()
        .filter_map(|path| {
            let differing = differing_artifacts(&path)?;
            Some((path, differing))
        })
        .collect();
    assert!(built.len() >= 400, "only {} files open", built.len());

    let mut by_artifact: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (path, names) in &built {
        for name in names {
            by_artifact.entry(name).or_default().push(path);
        }
    }
    let report: Vec<String> = by_artifact
        .iter()
        .map(|(name, paths)| {
            format!(
                "{name}: differs between two builds of {} files, such as {}",
                paths.len(),
                paths.iter().take(3).copied().collect::<Vec<_>>().join(", ")
            )
        })
        .collect();
    assert!(report.is_empty(), "{report:#?}");
}
