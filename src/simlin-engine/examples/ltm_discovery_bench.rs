// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Wall-clock comparison of LTM discovery's candidate generators --
//! union-graph circuit enumeration (the production default) against the
//! shortest-path fallback under each of its weight formulations -- on real
//! models.
//!
//! For each model (positional args; defaults to C-LEARN v77 and WRLD3-03):
//! compile with LTM discovery, simulate once, then run
//! `discover_loops_with_candidate_gen` under each generator and report loop
//! counts, the completeness/truncation flags, and per-phase timing. This is
//! the timing instrument; recall of the exact enumeration is a separate
//! question and a separate harness.
//!
//! Usage:
//!   cargo run --release --example ltm_discovery_bench [model.mdl ...]

use std::time::Instant;

use salsa::Setter;
use simlin_engine::db::{
    SimlinDb, causal_graph_from_element_edges, compile_project_incremental,
    model_element_causal_edges, model_ltm_variables, project_datamodel_dims,
    sync_from_datamodel_incremental,
};
use simlin_engine::ltm_finding::{CandidateGen, FallbackConfig, FallbackWeight};
use simlin_engine::{canonicalize, open_vensim, open_xmile};

fn main() {
    let mut models: Vec<String> = std::env::args().skip(1).collect();
    if models.is_empty() {
        let base = format!("{}/../../test", env!("CARGO_MANIFEST_DIR"));
        models = vec![
            format!("{base}/xmutil_test_models/C-LEARN v77 for Vensim.mdl"),
            format!("{base}/metasd/WRLD3-03/wrld3-03.mdl"),
        ];
    }

    for path in &models {
        println!("==> {path}");
        let contents = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                println!("  skipped (read error: {e})");
                continue;
            }
        };
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

        let t0 = Instant::now();
        let compiled = compile_project_incremental(&db, source_project, &canonical_name).unwrap();
        let compile_s = t0.elapsed().as_secs_f64();
        let t0 = Instant::now();
        let mut vm = simlin_engine::Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();
        let results = vm.into_results();
        let sim_s = t0.elapsed().as_secs_f64();

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
        let expansion = simlin_engine::analysis::build_link_expansion_context(
            &db,
            source_model,
            source_project,
        );
        let ports = simlin_engine::analysis::build_sub_model_output_ports(&db, source_project);

        println!(
            "  compile {compile_s:.2}s | LTM sim {sim_s:.2}s | saved steps {} | stocks {}",
            results.step_count,
            stocks.len()
        );

        let generators = [
            ("enumeration (Auto)      ", CandidateGen::Auto),
            (
                "fallback ClampedLogAbs  ",
                CandidateGen::FallbackOnly(FallbackConfig::with_weight(
                    FallbackWeight::ClampedLogAbs,
                )),
            ),
            (
                "fallback RelativeLink   ",
                CandidateGen::FallbackOnly(FallbackConfig::with_weight(
                    FallbackWeight::RelativeLinkScore,
                )),
            ),
            (
                "fallback HopCount       ",
                CandidateGen::FallbackOnly(FallbackConfig::with_weight(FallbackWeight::HopCount)),
            ),
        ];
        for (label, generator) in generators {
            let t0 = Instant::now();
            let found = simlin_engine::ltm_finding::discover_loops_with_candidate_gen(
                &results,
                &causal_graph,
                &stocks,
                &ltm_vars.vars,
                dm_dims,
                &expansion,
                &ports,
                None,
                generator,
            )
            .unwrap();
            let elapsed = t0.elapsed().as_secs_f64();
            // `universe` and `retained` are what the "Measured" tables in
            // docs/design-plans/2026-08-17-ltm-discovery-exact.md call
            // Circuits and Survivors; printing them here keeps those columns
            // readable off one run rather than re-derived.
            println!(
                "  {label}: {elapsed:>7.3}s | loops {:>4} | retained {:>5} | universe {:>7} | \
                 enumeration_complete {} | truncated {} | agg_trunc {}",
                found.loops.len(),
                found.retained_loops,
                found
                    .universe_loops
                    .map_or_else(|| "--".to_string(), |n| n.to_string()),
                found.enumeration_complete,
                found.truncated,
                found.agg_recovery_truncated,
            );
        }
        println!();
    }
}
