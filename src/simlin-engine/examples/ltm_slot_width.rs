// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The two numbers `simulate::clearn_ltm_var_count_guardrail` pins: a model's
//! emitted LTM variable COUNT and its per-step result-row WIDTH in slots (the
//! GH #654 resource, against the VM's 65,536 u16 slot ceiling).
//!
//! The guard's rustdoc requires re-measuring BOTH whenever the count moves, and
//! the width is not derivable from the count -- an arrayed variable occupies one
//! slot per element. This is the harness that produces them, so the numbers in
//! that rustdoc are regenerable rather than folklore.
//!
//! Usage:
//!   cargo run --release -p simlin-engine --example ltm_slot_width
//!   LTM_WIDTH_MODEL=path/to/model.mdl cargo run --release ... --example ltm_slot_width

use std::path::PathBuf;

use simlin_engine::db::{
    SimlinDb, model_ltm_variables, set_project_ltm_enabled, sync_from_datamodel_incremental,
};
use simlin_engine::queue_compile::compile_sim;
use simlin_engine::{open_vensim, open_xmile};

fn main() {
    let model_path = std::env::var("LTM_WIDTH_MODEL")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl")
        });

    let contents = std::fs::read_to_string(&model_path).expect("read model");
    let datamodel = if model_path.extension().is_some_and(|e| e == "mdl") {
        open_vensim(&contents).expect("import vensim model")
    } else {
        open_xmile(&mut contents.as_bytes()).expect("import xmile model")
    };
    println!("model: {}", model_path.display());

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);

    let total: usize = sync
        .models
        .values()
        .map(|m| {
            model_ltm_variables(&db, m.source_model, sync.project)
                .vars
                .len()
        })
        .sum();
    println!("emitted LTM variables: {total}");

    let main_name = datamodel
        .models
        .iter()
        .find(|m| m.name == "main")
        .map(|m| m.name.clone())
        .unwrap_or_else(|| datamodel.models[0].name.clone());
    let build = compile_sim(&mut db, sync.project, &datamodel, &main_name).expect("compile");
    let width = build.compiled.n_slots();
    println!("per-step result-row width: {width} slots");
    println!("free against the 65,536-slot ceiling: {}", 65536 - width);
}
