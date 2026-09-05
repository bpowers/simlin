// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Dump every corpus model's dependency graph -- the three runlists, the
//! cycle verdict, the resolved recurrence SCCs and both transitive
//! dependency maps -- to one text file, so a change to
//! `model_dependency_graph` is checked by diffing the dump from the base
//! tree against the dump from the changed tree rather than by trusting that
//! the simulation numbers happened not to move (a runlist that changes order
//! without changing any number is invisible to the simulate corpus; see
//! "Measuring a change" in docs/design/engine-performance.md).
//!
//! Usage, from `src/simlin-engine`:
//!
//!   cargo run --example depgraph_dump -- /tmp/depgraph.base.txt   # on base
//!   cargo run --example depgraph_dump -- /tmp/depgraph.new.txt    # on the tree
//!   diff /tmp/depgraph.base.txt /tmp/depgraph.new.txt
//!
//! Every `.xmile`, `.stmx` and `.mdl` under `test/` is opened; a model the
//! importer refuses is recorded as such and skipped. Each model is dumped
//! under the empty module-input set (the wiring the diagnostic pass and the
//! root assembly use), with the maps sorted by key so the file is a function
//! of the models alone.
use simlin_engine::db::{
    ModuleInputSet, SimlinDb, model_dependency_graph, sync_from_datamodel_incremental,
};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, out);
        } else if let Some(ext) = p.extension().and_then(|e| e.to_str())
            && matches!(ext, "xmile" | "stmx" | "mdl")
        {
            out.push(p);
        }
    }
}

fn main() {
    let root = format!("{}/../../test", env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    walk(Path::new(&root), &mut files);
    let out_path = std::env::args().nth(1).expect("output path");
    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).unwrap());
    let mut n = 0;
    for path in files {
        let rel = path.strip_prefix(&root).unwrap().display().to_string();
        let dm = match path.extension().and_then(|e| e.to_str()) {
            Some("mdl") => match std::fs::read_to_string(&path) {
                Ok(s) => simlin_engine::open_vensim(&s),
                Err(_) => continue,
            },
            _ => match std::fs::File::open(&path) {
                Ok(f) => simlin_engine::open_xmile(&mut std::io::BufReader::new(f)),
                Err(_) => continue,
            },
        };
        let dm = match dm {
            Ok(dm) => dm,
            Err(e) => {
                writeln!(out, "== {rel}: open failed: {e}").unwrap();
                continue;
            }
        };
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &dm, None);
        let project = sync.project;
        let models: BTreeMap<String, _> = project
            .models(&db)
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        for (name, model) in models {
            let g = model_dependency_graph(&db, model, project, ModuleInputSet::empty(&db));
            writeln!(out, "== {rel} :: {name}").unwrap();
            writeln!(out, "cycle: {:?}", g.cycle_variables).unwrap();
            writeln!(out, "initials: {:?}", g.runlist_initials).unwrap();
            writeln!(out, "flows: {:?}", g.runlist_flows).unwrap();
            writeln!(out, "stocks: {:?}", g.runlist_stocks).unwrap();
            for scc in &g.resolved_sccs {
                writeln!(
                    out,
                    "scc {:?}: {:?} order {:?}",
                    scc.phase, scc.members, scc.element_order
                )
                .unwrap();
            }
            let mut dt: Vec<_> = g.dt_dependencies.iter().collect();
            dt.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
            for (k, v) in dt {
                writeln!(
                    out,
                    "dt {} -> {:?}",
                    k.as_str(),
                    v.iter().map(|i| i.as_str()).collect::<Vec<_>>()
                )
                .unwrap();
            }
            let mut init: Vec<_> = g.initial_dependencies.iter().collect();
            init.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
            for (k, v) in init {
                writeln!(
                    out,
                    "init {} -> {:?}",
                    k.as_str(),
                    v.iter().map(|i| i.as_str()).collect::<Vec<_>>()
                )
                .unwrap();
            }
            n += 1;
        }
    }
    eprintln!("dumped {n} models");
}
