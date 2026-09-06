// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Every LTM link score a model DECLINES to emit, bucketed by the reason the
//! generator gave.
//!
//! `examples/ltm_fragment_failures.rs` counts fragments that fail to COMPILE;
//! this counts the ones never generated at all -- the `PartialEquationError`
//! family (unprojectable dep, rank-like partial, unfreezable partial, bare
//! reducer feeder, parse failure) plus the GH #758 loud skip. Those are
//! invisible to the fragment count precisely because nothing was emitted.
//!
//! Usage:
//!   cargo run --release -p simlin-engine --example ltm_declined_edges
//!   LTM_DECLINE_MODEL=path/to/model.mdl cargo run --release ... --example ltm_declined_edges

use std::collections::BTreeMap;
use std::path::PathBuf;

use simlin_engine::db::{SimlinDb, collect_all_diagnostics, sync_from_datamodel_incremental};
use simlin_engine::{open_vensim, open_xmile};

/// Which decline this diagnostic reports, keyed off the message's own wording
/// (the messages are the only channel `collect_all_diagnostics` exposes).
fn bucket(msg: &str) -> Option<&'static str> {
    if !msg.contains("could not be generated") && !msg.contains("no link score") {
        return None;
    }
    let kinds = [
        (
            "cannot be projected onto that target element",
            "unprojectable-dep",
        ),
        ("array-producing", "rank-like-partial"),
        ("freeze an array slice", "unfreezable-partial"),
        ("inside an array-reducer argument", "bare-reducer-feeder"),
        ("did not parse", "parse-failure"),
    ];
    for (needle, name) in kinds {
        if msg.contains(needle) {
            return Some(name);
        }
    }
    Some("other")
}

fn main() {
    let model_path = std::env::var("LTM_DECLINE_MODEL")
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

    let diags = collect_all_diagnostics(&db, sync.project, simlin_engine::db::LtmOverlay::On);
    let mut by_bucket: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for d in &diags {
        let msg = format!("{:?}", d.error);
        if let Some(b) = bucket(&msg) {
            // The link-score variable name is the quoted ident right after
            // "variable '".
            let name = msg
                .split_once("variable '")
                .and_then(|(_, rest)| rest.split_once('\''))
                .map(|(n, _)| n.to_string())
                .unwrap_or_else(|| msg.clone());
            // The offending dep / equation text, the second quoted run.
            let detail = msg
                .split_once("dependency '")
                .or_else(|| msg.split_once("equation '"))
                .and_then(|(_, rest)| rest.split_once('\''))
                .map(|(d, _)| d.to_string())
                .unwrap_or_default();
            by_bucket
                .entry(b)
                .or_default()
                .push(format!("{name}   [{detail}]"));
        }
    }

    let total: usize = by_bucket.values().map(Vec::len).sum();
    println!("declined link scores: {total}");
    for (b, names) in &by_bucket {
        println!("\n=== {b}: {} ===", names.len());
        let mut names = names.clone();
        names.sort();
        for n in &names {
            println!("  {n}");
        }
    }
}
