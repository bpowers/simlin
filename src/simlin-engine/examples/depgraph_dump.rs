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
//! root assembly use), then under every distinct bound-port set an instance
//! of it binds anywhere in the project -- the production enumeration
//! (`enumerate_module_instances`) run from every model as a root -- so the
//! sweep covers the wired arm, where a with-inputs relation meets the
//! no-input recurrence resolution. The maps are sorted by key so the file is
//! a function of the models alone.
use simlin_engine::common::{Canonical, Ident};
use simlin_engine::db::{
    ModuleInputSet, SimlinDb, SourceModel, SourceProject, enumerate_module_instances,
    model_dependency_graph, project_module_graph, sync_from_datamodel_incremental,
};
use std::collections::{BTreeMap, BTreeSet};
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

/// One model's graph under one wiring. `wiring` is empty for the no-input
/// set, so that section's header is exactly the model's name.
fn dump_graph(
    out: &mut impl Write,
    db: &SimlinDb,
    header: &str,
    wiring: &str,
    model: SourceModel,
    project: SourceProject,
    module_inputs: ModuleInputSet<'_>,
) {
    let g = model_dependency_graph(db, model, project, module_inputs);
    writeln!(out, "== {header}{wiring}").unwrap();
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
}

fn main() {
    let root = format!("{}/../../test", env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    walk(Path::new(&root), &mut files);
    let out_path = std::env::args().nth(1).expect("output path");
    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).unwrap());
    let mut n = 0;
    let mut n_wired = 0;
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

        // Every bound-port set each model is instantiated under, from every
        // model as a root: the union over roots is every wiring the project
        // can compile a model under. A root inside a module cycle is skipped
        // exactly as assembly refuses it. Notes about a root go after the
        // model sections, so the sections keep their position.
        let mut wired: BTreeMap<Ident<Canonical>, BTreeSet<BTreeSet<Ident<Canonical>>>> =
            BTreeMap::new();
        let mut notes: Vec<String> = Vec::new();
        for root_model in models.keys() {
            if project_module_graph(&db, project)
                .cycle_error_from(root_model)
                .is_some()
            {
                notes.push(format!(
                    "== {rel} :: {root_model}: module cycle, instances not enumerated"
                ));
                continue;
            }
            match enumerate_module_instances(&db, project, root_model) {
                Ok(instances) => {
                    for (target, sets) in instances {
                        wired.entry(target).or_default().extend(sets);
                    }
                }
                Err(e) => notes.push(format!(
                    "== {rel} :: {root_model}: instance enumeration failed: {e}"
                )),
            }
        }

        for (name, model) in models {
            let header = format!("{rel} :: {name}");
            dump_graph(
                &mut out,
                &db,
                &header,
                "",
                model,
                project,
                ModuleInputSet::empty(&db),
            );
            n += 1;
            let Some(sets) = wired.get(&Ident::<Canonical>::new(&name)) else {
                continue;
            };
            for inputs in sets.iter().filter(|inputs| !inputs.is_empty()) {
                let names: Vec<&str> = inputs.iter().map(|i| i.as_str()).collect();
                let wiring = format!(" [inputs {}]", names.join(", "));
                dump_graph(
                    &mut out,
                    &db,
                    &header,
                    &wiring,
                    model,
                    project,
                    ModuleInputSet::from_canonical_set(&db, inputs),
                );
                n_wired += 1;
            }
        }
        for note in notes {
            writeln!(out, "{note}").unwrap();
        }
    }
    eprintln!("dumped {n} models, plus {n_wired} wired instances");
}
