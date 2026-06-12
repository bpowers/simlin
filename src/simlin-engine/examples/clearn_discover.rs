// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! LTM discovery-feasibility benchmark harness (GH #647).
//!
//! Runs the full LTM-discovery pipeline (compile, simulate, strongest-path
//! discovery DFS) on a large model -- C-LEARN v77 by default -- with phase
//! timing, so the per-stage cost and the discovery DFS feasibility can be
//! measured against real link scores.
//!
//! Environment:
//!   CLEARN_MODEL            override the model path (.mdl, .stmx/.xmile)
//!   CLEARN_SKIP_DISCOVERY   stop before the discovery DFS
//!   CLEARN_DISCOVER_STEPS=N truncate discovery to the first N saved steps
//!   CLEARN_CAP_SCORES       clamp |link score| <= 1 before discovery
//!   CLEARN_GRAPH_STATS      report search-graph structure (nodes/edges/SCCs)
//!                           and per-step score distribution, then exit
//!   CLEARN_DUMP_LINK=NEEDLE dump link-score columns matching NEEDLE

use std::io::Write;
use std::time::Instant;

use salsa::Setter;
use simlin_engine::analysis::analyze_model;
use simlin_engine::db::{
    SimlinDb, causal_graph_from_element_edges, compile_project_incremental,
    model_element_causal_edges, model_ltm_variables, project_datamodel_dims,
    sync_from_datamodel_incremental,
};
use simlin_engine::{canonicalize, open_vensim, open_xmile};

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
    let path = std::env::var("CLEARN_MODEL").unwrap_or_else(|_| {
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
    println!("model: {path}");

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let source_project = sync.project;
    let root_name = datamodel
        .models
        .first()
        .map(|m| m.name.as_str())
        .unwrap_or("main");
    let canonical_name = canonicalize(root_name).into_owned();

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

    // Spot-check: dump a specific simple link score plus its endpoint
    // variables so the ceteris-paribus formula can be verified by hand.
    // Set CLEARN_DUMP_LINK to a link-score column name (or a substring).
    if let Ok(needle) = std::env::var("CLEARN_DUMP_LINK") {
        let col = |name: &str| -> Option<Vec<f64>> {
            results.offsets.iter().find_map(|(n, &off)| {
                if n.as_str() == name {
                    Some(
                        (0..results.step_count)
                            .map(|s| results.data[s * results.step_size + off])
                            .collect(),
                    )
                } else {
                    None
                }
            })
        };
        // "NONZERO_REAL" is a special needle: real (non-helper) link-score
        // columns carrying a finite non-zero value somewhere in the run.
        let matching: Vec<String> = if needle == "NONZERO_REAL" {
            results
                .offsets
                .iter()
                .filter(|(n, off)| {
                    n.as_str()
                        .starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}")
                        && (0..results.step_count).any(|s| {
                            let v = results.data[s * results.step_size + **off];
                            v.is_finite() && v != 0.0
                        })
                })
                .map(|(n, _)| n.as_str().to_string())
                .collect()
        } else {
            results
                .offsets
                .iter()
                .filter(|(n, _)| n.as_str().contains(&needle))
                .map(|(n, _)| n.as_str().to_string())
                .collect()
        };
        println!("  columns matching {needle:?}: {}", matching.len());
        for name in matching.iter().take(8) {
            let series = col(name).unwrap();
            println!("    {} = {:?}", name, &series[..6.min(series.len())]);
        }

        // Offset-collision check: how many result slots have more than one
        // name mapped to them? A link-score column showing a model variable's
        // value means either the offsets map collides or the bytecode
        // double-writes a slot.
        let mut by_offset: std::collections::HashMap<usize, Vec<&str>> =
            std::collections::HashMap::new();
        for (n, &off) in results.offsets.iter() {
            by_offset.entry(off).or_default().push(n.as_str());
        }
        let collisions: Vec<(&usize, &Vec<&str>)> = by_offset
            .iter()
            .filter(|(_, names)| names.len() > 1)
            .collect();
        let ltm_collisions = collisions
            .iter()
            .filter(|(_, names)| names.iter().any(|n| n.contains("ltm")))
            .count();
        println!(
            "  offset collisions: {} slots with >1 name ({} involving LTM vars) of {} total slots",
            collisions.len(),
            ltm_collisions,
            by_offset.len()
        );
        for (off, names) in collisions.iter().take(6) {
            println!(
                "    slot {}: {:?}",
                off,
                names.iter().take(3).collect::<Vec<_>>()
            );
        }
    }

    // Degenerate-workload guard: the strongest-path DFS prunes on link-score
    // magnitude, so if the LTM link-score columns are all zero/NaN the discovery
    // benchmark is meaningless. Count how many link-score columns carry at least
    // one finite non-zero value across the run.
    let mut link_cols = 0usize;
    let mut link_cols_nonzero = 0usize;
    let mut suspect_zero_cols: Vec<&str> = Vec::new();
    let varies = |off: usize| -> bool {
        let first = results.data[results.step_size + off];
        (2..results.step_count).any(|step| {
            let v = results.data[step * results.step_size + off];
            v.is_finite() && (v - first).abs() > 1e-12
        })
    };
    for (name, &off) in results.offsets.iter() {
        let name_str = name.as_str();
        if !name_str.contains("link_score") {
            continue;
        }
        link_cols += 1;
        let any_nonzero = (0..results.step_count).any(|step| {
            let v = results.data[step * results.step_size + off];
            v.is_finite() && v != 0.0
        });
        if any_nonzero {
            link_cols_nonzero += 1;
        } else {
            // An always-zero link score is legitimate when its source never
            // changes (constants have score 0 by definition). If BOTH
            // endpoints vary over the run but the score is still identically
            // zero, the link-score fragment is suspect (e.g. silently failed
            // to compile -- the GH #587 failure mode).
            let suffix = name_str
                .strip_prefix("$\u{205A}ltm\u{205A}link_score\u{205A}")
                .unwrap_or("");
            if let Some((from_str, to_str)) = suffix.split_once('\u{2192}') {
                let base = |s: &str| s.split('[').next().unwrap_or(s).to_string();
                let from_off = results
                    .offsets
                    .get(&simlin_engine::common::Ident::new(&base(from_str)));
                let to_off = results
                    .offsets
                    .get(&simlin_engine::common::Ident::new(&base(to_str)));
                if let (Some(&fo), Some(&to_o)) = (from_off, to_off)
                    && varies(fo)
                    && varies(to_o)
                {
                    suspect_zero_cols.push(name_str);
                }
            }
        }
    }
    println!(
        "  LTM link_score columns: {link_cols} (with a finite non-zero value: {link_cols_nonzero})"
    );
    if !suspect_zero_cols.is_empty() {
        suspect_zero_cols.sort();
        println!(
            "  SUSPECT always-zero link scores (both endpoints vary): {}",
            suspect_zero_cols.len()
        );
        for name in suspect_zero_cols.iter().take(15) {
            println!("    {name}");
        }
    }

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
    // Per-variable declared dims + dimension-mapping context for the A2A
    // from-node projection (GH #754), built through the production decision.
    let expansion =
        simlin_engine::analysis::build_link_expansion_context(&db, source_model, source_project);
    println!(
        "  element-graph stocks: {}, ltm synthetic vars: {}",
        stocks.len(),
        ltm_vars.vars.len()
    );

    // Search-graph structure / score-distribution diagnostics (GH #647): how
    // big is the graph the per-timestep DFS must traverse, where is its cyclic
    // core, and how do the per-step scores distribute relative to the pruning
    // assumptions (products shrink along paths; few exact ties)?
    if std::env::var("CLEARN_GRAPH_STATS").is_ok() {
        let last = results.step_count - 1;
        let sample_steps: Vec<usize> = vec![1, 5, last / 4, last / 2, 3 * last / 4, last]
            .into_iter()
            .filter(|&s| s > 0)
            .collect();
        let stats = phase("discovery graph stats", || {
            simlin_engine::ltm_finding::discovery_graph_stats(
                &results,
                &stocks,
                &ltm_vars.vars,
                dm_dims,
                &expansion,
                &sample_steps,
            )
        });
        println!(
            "  search graph: {} nodes, {} edges, {} stocks",
            stats.n_nodes, stats.n_edges, stats.n_stocks
        );
        let topo_core: usize = stats.topology_scc_sizes.iter().sum();
        println!(
            "  static topology cyclic core: {} nodes in {} multi-node SCCs (largest: {:?}); {} of {} stocks inside",
            topo_core,
            stats.topology_scc_sizes.len(),
            &stats.topology_scc_sizes[..stats.topology_scc_sizes.len().min(8)],
            stats.stocks_in_cyclic_core,
            stats.n_stocks,
        );
        for s in &stats.step_stats {
            let nz_core: usize = s.nonzero_scc_sizes.iter().sum();
            println!(
                "  step {:>3}: zero {} | unit {} | (0,1) {} | >1 {} | max |s| {:.3e} | nonzero-SCC core {} nodes in {} SCCs (largest: {:?}), {} stocks inside",
                s.step,
                s.zero_edges,
                s.unit_edges,
                s.sub_unit_edges,
                s.super_unit_edges,
                s.max_abs_score,
                nz_core,
                s.nonzero_scc_sizes.len(),
                &s.nonzero_scc_sizes[..s.nonzero_scc_sizes.len().min(8)],
                s.stocks_in_nonzero_core,
            );
        }
        println!("\n[CLEARN_GRAPH_STATS set: stopping before discovery DFS]");
        return;
    }

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

    let sub_model_ports =
        simlin_engine::analysis::build_sub_model_output_ports(&db, source_project);
    let found = phase("strongest-path discovery DFS", || {
        // `budget: None` runs the full DFS to completion -- this harness exists
        // to time the whole discovery sweep, so it must never truncate.
        simlin_engine::ltm_finding::discover_loops_with_graph(
            &results,
            &causal_graph,
            &stocks,
            &ltm_vars.vars,
            dm_dims,
            &expansion,
            &sub_model_ports,
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
