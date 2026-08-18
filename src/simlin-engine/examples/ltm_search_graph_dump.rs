// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Dump the exact element-level search graph LTM discovery consumes, with
//! per-edge link-score series, so external tooling (notebooks, audits) can
//! exhaustively enumerate cycles and compare against discovery's reported
//! loops on the IDENTICAL graph.
//!
//! The edge set comes from the public [`simlin_engine::ltm_finding::link_score_offsets`]
//! (the production `parse_link_offsets` expansion), never a re-derivation --
//! see that function's rustdoc for why a second implementation would make the
//! audit instrument disagree with the thing it audits.
//!
//! Output (JSON to stdout or `LTM_DUMP_OUT`):
//!   {
//!     "model": "<path>",
//!     "step_count": N,           // saved steps
//!     "stocks": ["...", ...],    // the fallback's seed nodes, engine order
//!     "edges": [{"from": "...", "to": "...", "scores": [f64; N]}, ...],
//!     "discovered": [{"id": "...", "nodes": ["...", ...]}, ...]
//!   }
//!
//! Environment:
//!   LTM_DUMP_MODEL   override the model path (default: C-LEARN v77)
//!   LTM_DUMP_OUT     output file path (default: stdout)

use salsa::Setter;
use simlin_engine::db::{
    SimlinDb, causal_graph_from_element_edges, compile_project_incremental,
    model_element_causal_edges, model_ltm_variables, project_datamodel_dims,
    sync_from_datamodel_incremental,
};
use simlin_engine::{canonicalize, open_vensim, open_xmile};

fn main() {
    let path = std::env::var("LTM_DUMP_MODEL").unwrap_or_else(|_| {
        format!(
            "{}/../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl",
            env!("CARGO_MANIFEST_DIR")
        )
    });
    let contents = std::fs::read_to_string(&path).unwrap();
    let datamodel = if path.ends_with(".mdl") {
        open_vensim(&contents).unwrap()
    } else {
        open_xmile(&mut contents.as_bytes()).unwrap()
    };

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let source_project = sync.project;
    let root_name = datamodel
        .models
        .first()
        .map(|m| m.name.as_str())
        .unwrap_or("main");
    let canonical_name = canonicalize(root_name).into_owned();

    source_project.set_ltm_enabled(&mut db).to(true);
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let compiled = compile_project_incremental(&db, source_project, &canonical_name).unwrap();
    let mut vm = simlin_engine::Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let source_model = source_project
        .models(&db)
        .get(canonical_name.as_str())
        .copied()
        .unwrap();

    let element_edges = model_element_causal_edges(&db, source_model, source_project);
    let causal_graph = causal_graph_from_element_edges(element_edges);
    let stocks: Vec<_> = element_edges
        .stocks
        .iter()
        .map(|s| simlin_engine::common::Ident::new(s))
        .collect();
    let ltm_vars = model_ltm_variables(&db, source_model, source_project);
    let dm_dims = project_datamodel_dims(&db, source_project);
    let expansion =
        simlin_engine::analysis::build_link_expansion_context(&db, source_model, source_project);

    let link_offsets = simlin_engine::ltm_finding::link_score_offsets(
        &results,
        &ltm_vars.vars,
        dm_dims,
        &expansion,
    );

    // Run production discovery on the same inputs for cross-checking.
    let sub_model_ports =
        simlin_engine::analysis::build_sub_model_output_ports(&db, source_project);
    let found = simlin_engine::ltm_finding::discover_loops_with_graph(
        &results,
        &causal_graph,
        &stocks,
        &ltm_vars.vars,
        dm_dims,
        &expansion,
        &sub_model_ports,
        None,
    )
    .unwrap();

    let edges: Vec<serde_json::Value> = link_offsets
        .iter()
        .map(|((from, to), offset)| {
            let scores: Vec<f64> = (0..results.step_count)
                .map(|s| results.data[s * results.step_size + offset])
                .collect();
            serde_json::json!({
                "from": from.as_str(),
                "to": to.as_str(),
                "scores": scores,
            })
        })
        .collect();

    let discovered: Vec<serde_json::Value> = found
        .loops
        .iter()
        .map(|l| {
            // The reported cycle's node sequence (synthetic agg nodes already
            // trimmed by the engine); the audit trims synthetics from its own
            // enumeration before matching.
            let nodes: Vec<&str> = l.loop_info.links.iter().map(|k| k.from.as_str()).collect();
            serde_json::json!({
                "id": l.loop_info.id,
                "polarity": format!("{:?}", l.loop_info.polarity),
                "avg_abs_score": l.avg_abs_score,
                "nodes": nodes,
            })
        })
        .collect();

    let out = serde_json::json!({
        "model": path,
        "step_count": results.step_count,
        "stocks": stocks.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        "edges": edges,
        "discovered": discovered,
    });

    let text = serde_json::to_string(&out).unwrap();
    match std::env::var("LTM_DUMP_OUT") {
        Ok(p) => {
            std::fs::write(&p, text).unwrap();
            eprintln!(
                "wrote {} ({} edges, {} stocks, {} discovered loops, {} steps)",
                p,
                link_offsets.len(),
                stocks.len(),
                found.loops.len(),
                results.step_count
            );
        }
        Err(_) => println!("{text}"),
    }
}
