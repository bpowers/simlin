// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Every LTM variable name a model emits, `model<TAB>name`, sorted -- the
//! instrument behind `simulate::clearn_ltm_var_count_guardrail`'s derivation.
//!
//! The guardrail pins a COUNT, which says a number moved but not which names
//! moved or in which direction. Diffing two runs of this does:
//!
//! ```text
//! cargo run --release -p simlin-engine --example ltm_var_dump > after.txt
//! # (revert the change under test)
//! cargo run --release -p simlin-engine --example ltm_var_dump > before.txt
//! comm -13 <(sort before.txt) <(sort after.txt)   # added
//! comm -23 <(sort before.txt) <(sort after.txt)   # removed
//! ```
//!
//! That is how the MDL apply-to-all import fix was shown to be strictly
//! additive (315 added, 0 removed) rather than a wash of gains and losses.
use simlin_engine::db::{SimlinDb, model_ltm_variables, sync_from_datamodel_incremental};
fn main() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl"
    );
    let contents = std::fs::read_to_string(path).expect("read model");
    let datamodel = simlin_engine::open_vensim(&contents).expect("import");
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let mut names: Vec<String> = Vec::new();
    for (model_name, m) in sync.models.iter() {
        let ltm = model_ltm_variables(&db, m.source_model, sync.project);
        names.extend(ltm.vars.iter().map(|v| format!("{model_name}\t{}", v.name)));
    }
    names.sort_unstable();
    for n in &names {
        println!("{n}");
    }
}
