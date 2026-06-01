// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! One-off: run the engine's high-level `analyze_model` (strongest-path LTM
//! discovery) on C-LEARN and report how many feedback loops + dominant periods
//! it finds. Used to demonstrate that the discovery data exists in the engine
//! but is not exposed through the libsimlin FFI that pysimlin consumes.

use std::io::Write;
use std::time::Instant;

use salsa::Setter;
use simlin_engine::analysis::analyze_model;
use simlin_engine::db::{
    SimlinDb, causal_graph_from_element_edges, compile_project_incremental,
    model_element_causal_edges, model_ltm_variables, project_datamodel_dims,
    sync_from_datamodel_incremental,
};
use simlin_engine::{canonicalize, open_vensim};

fn phase<T>(name: &str, f: impl FnOnce() -> T) -> T {
    print!("{name:<28} ... ");
    std::io::stdout().flush().ok();
    let t0 = Instant::now();
    let out = f();
    println!("{:>9.2} s", t0.elapsed().as_secs_f64());
    std::io::stdout().flush().ok();
    out
}

fn main() {
    let path = format!(
        "{}/../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl",
        env!("CARGO_MANIFEST_DIR")
    );
    let contents = std::fs::read_to_string(&path).unwrap();
    let datamodel = open_vensim(&contents).unwrap();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let source_project = sync.project;
    let canonical_name = canonicalize("main").into_owned();

    // Decompose the analyze_model pipeline so we can see whether the cost is in
    // the LTM *simulation* (which the wasm backend could accelerate) or the
    // strongest-path *discovery DFS* (pure Rust graph search, wasm-agnostic).
    source_project.set_ltm_enabled(&mut db).to(true);
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let compiled = phase("compile (salsa, +LTM)", || {
        compile_project_incremental(&db, source_project, &canonical_name).unwrap()
    });

    let mut vm = phase("Vm::new", || simlin_engine::Vm::new(compiled).unwrap());
    phase("LTM sim run_to_end (VM)", || vm.run_to_end().unwrap());
    let mut results = vm.into_results();
    println!("  saved steps: {}", results.step_count);
    println!("  result slots/step: {}", results.step_size);

    // Degenerate-workload guard: the strongest-path DFS prunes on link-score
    // magnitude, so if the LTM link-score columns are all zero/NaN the discovery
    // benchmark is meaningless. Count how many link-score columns carry at least
    // one finite non-zero value across the run.
    let mut link_cols = 0usize;
    let mut link_cols_nonzero = 0usize;
    for (name, &off) in results.offsets.iter() {
        if !name.as_str().contains("link_score") {
            continue;
        }
        link_cols += 1;
        let any_nonzero = (0..results.step_count).any(|step| {
            let v = results.data[step * results.step_size + off];
            v.is_finite() && v != 0.0
        });
        if any_nonzero {
            link_cols_nonzero += 1;
        }
    }
    println!(
        "  LTM link_score columns: {link_cols} (with a finite non-zero value: {link_cols_nonzero})"
    );

    let source_model = source_project
        .models(&db)
        .get(canonical_name.as_str())
        .copied()
        .unwrap();

    let element_edges = phase("build element causal graph", || {
        model_element_causal_edges(&db, source_model, source_project)
    });
    let causal_graph = causal_graph_from_element_edges(element_edges);
    let stocks: Vec<_> = element_edges
        .stocks
        .iter()
        .map(|s| simlin_engine::common::Ident::new(s))
        .collect();
    let ltm_vars = model_ltm_variables(&db, source_model, source_project);
    let dm_dims = project_datamodel_dims(&db, source_project);
    println!(
        "  element-graph stocks: {}, ltm synthetic vars: {}",
        stocks.len(),
        ltm_vars.vars.len()
    );

    if std::env::var("CLEARN_SKIP_DISCOVERY").is_ok() {
        println!("\n[CLEARN_SKIP_DISCOVERY set: stopping before discovery DFS]");
        return;
    }

    // Optional: truncate discovery to the first N saved timesteps so the
    // per-timestep cost can be measured on the real (huge) C-LEARN workload
    // without waiting for all 251 steps. Discovery is per-step independent.
    if let Some(n) = std::env::var("CLEARN_DISCOVER_STEPS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        let n = n.min(results.step_count);
        let keep = n * results.step_size;
        let mut data = results.data.into_vec();
        data.truncate(keep);
        results.data = data.into_boxed_slice();
        results.step_count = n;
        println!("  [discovery truncated to first {n} timesteps]");
    }

    if std::env::var("CLEARN_CAP_SCORES").is_ok() {
        // Experiment: clamp every LTM link-score magnitude to <= 1 before
        // discovery. The strongest-path DFS prunes via `score < best_score`,
        // which assumes path-score products SHRINK as paths extend. Link scores
        // > 1 (e.g. the ~5.2e7 macro-internal scores) make products GROW, which
        // can defeat that pruning and cause the per-timestep blowup. If capping
        // restores fast discovery, the pruning-defeat hypothesis holds.
        let link_offs: Vec<usize> = results
            .offsets
            .iter()
            .filter(|(n, _)| n.as_str().contains("link_score"))
            .map(|(_, &o)| o)
            .collect();
        let (step_size, step_count) = (results.step_size, results.step_count);
        let mut capped = 0usize;
        for step in 0..step_count {
            for &off in &link_offs {
                let idx = step * step_size + off;
                let v = results.data[idx];
                if v.is_finite() && v.abs() > 1.0 {
                    results.data[idx] = v.signum();
                    capped += 1;
                }
            }
        }
        println!("  [capped {capped} link-score values to |.|<=1]");
    }

    let found = phase("strongest-path discovery DFS", || {
        // `budget: None` runs the full DFS to completion -- this harness exists
        // to time the whole discovery sweep, so it must never truncate.
        simlin_engine::ltm_finding::discover_loops_with_graph(
            &results,
            &causal_graph,
            &stocks,
            &ltm_vars.vars,
            dm_dims,
            None,
        )
        .unwrap()
    });
    println!(
        "  discovered loops (raw): {} (truncated: {})",
        found.loops.len(),
        found.truncated
    );

    source_project.set_ltm_enabled(&mut db).to(false);
    source_project.set_ltm_discovery_mode(&mut db).to(false);

    println!("discovered loops: {}", found.loops.len());
    println!("time steps: {}", results.step_count);
    let _ = analyze_model; // kept imported for reference; phases above replicate it
}
