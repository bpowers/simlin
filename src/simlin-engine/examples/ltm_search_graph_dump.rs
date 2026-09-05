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
//!     "partitions": [["stock", ...], ...],  // cycle partitions, engine order
//!     "edges": [{"from": "...", "to": "...", "scores": [f64; N]}, ...],
//!     "enumeration_complete": bool,
//!     "universe_loops": usize|null,   // Some(n) iff enumeration_complete
//!     "retained_loops": usize,
//!     "truncated": bool,
//!     "agg_recovery_truncated": bool,
//!     "max_agg_petals": usize,        // production stitching caps, for the audit
//!     "cross_agg_loop_budget": usize,
//!     "discovered": [{"id": "...", "nodes": [...], "scores": [f64; N],
//!                     "rel_scores": [f64; N], "partition": usize|null}, ...]
//!   }
//!
//! `partitions` is the FULL cycle-partition list (`CausalGraph::
//! compute_cycle_partitions`), not the result-scoped `DiscoveryResult::
//! partitions` -- an external enumeration has to place every cycle it finds,
//! including the ones the engine never reported, so it needs the whole
//! stock-to-partition map rather than the subset the reported loops touched.
//! A discovered loop's `partition` is its index into this list (`null` for a
//! loop whose stocks resolve to no parent-level partition), so the audit's
//! grouping and the engine's are the same grouping.
//!
//! `scores` is the engine's signed raw loop-score series and `rel_scores` its
//! signed partition-relative series -- the statistic ranking and dominance are
//! read from -- so an external re-derivation can be differenced against both
//! step by step. The pair is what makes the module-override case checkable
//! rather than assumed: for a loop through a module instance the reported
//! `scores` are the per-exit-port override series and NOT the raw product of
//! the `edges` rows, so a consumer can detect that from the data instead of
//! having to know which nodes the engine treats as modules.
//!
//! **Non-finite score values are spelled explicitly, never `null`.** Every
//! score array (`edges[].scores`, `discovered[].scores`,
//! `discovered[].rel_scores`) encodes a finite value as a JSON number and a
//! non-finite one as the string `"nan"`, `"inf"`, or `"-inf"` -- `serde_json`
//! collapses NaN/Inf/-Inf to `null` by default, which erases the distinction
//! the loader needs: per `is_active` (the ONE activity rule both generators
//! and the totals share), an infinite score is ACTIVE -- a genuine divergent
//! signal that multiplies through a partition's totals -- while NaN is
//! INACTIVE and poisons any product it appears in. `null` cannot tell those
//! apart, so a consumer reading it either has to guess or treats every
//! non-finite step as inactive, which silently disagrees with the engine on
//! a divergent run. `notebooks/build_ltm_discovery_audit.py`'s loader decodes
//! the three spellings back to `float('nan')`/`float('inf')`/`float('-inf')`.
//!
//! Environment:
//!   LTM_DUMP_MODEL   override the model path (default: C-LEARN v77)
//!   LTM_DUMP_OUT     output file path (default: stdout)

use salsa::Setter;
use simlin_engine::db::{
    SimlinDb, causal_graph_from_element_edges_with_modules, compile_project_incremental,
    model_element_causal_edges, model_ltm_variables, project_datamodel_dims,
    sync_from_datamodel_incremental,
};
use simlin_engine::{canonicalize, open_vensim, open_xmile};

/// Encode one score value as JSON, spelling a non-finite value explicitly
/// (`"nan"` / `"inf"` / `"-inf"`) instead of letting `serde_json` collapse it
/// to `null` -- see the module doc for why the distinction matters to the
/// loader.
fn score_to_json(v: f64) -> serde_json::Value {
    if v.is_nan() {
        serde_json::Value::String("nan".to_string())
    } else if v == f64::INFINITY {
        serde_json::Value::String("inf".to_string())
    } else if v == f64::NEG_INFINITY {
        serde_json::Value::String("-inf".to_string())
    } else {
        serde_json::json!(v)
    }
}

/// [`score_to_json`] over a whole series.
fn scores_to_json(vs: &[f64]) -> Vec<serde_json::Value> {
    vs.iter().copied().map(score_to_json).collect()
}

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

    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let compiled = compile_project_incremental(
        &db,
        source_project,
        &canonical_name,
        simlin_engine::db::LtmOverlay::On,
    )
    .unwrap();
    let mut vm = simlin_engine::Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let source_model = source_project
        .models(&db)
        .get(canonical_name.as_str())
        .copied()
        .unwrap();

    let element_edges = model_element_causal_edges(&db, source_model, source_project);
    // The PRODUCTION constructor (`analysis::analyze_model` uses the
    // identical call): the bare `causal_graph_from_element_edges` leaves the
    // module sub-graphs and variable map empty, which silently disables the
    // discovery-mode per-exit-port pathway recompute this dump's own module-
    // override doc above describes (GH #698) -- on a module-bearing model
    // that made this instrument run a DIFFERENT discovery path than
    // `Model.analyze()`.
    let causal_graph = causal_graph_from_element_edges_with_modules(
        &db,
        source_model,
        source_project,
        element_edges,
    );
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

    // The engine's own cycle partitions -- the grouping every relative score
    // is normalized within. Computed here (not re-derived in the consumer)
    // for the same reason the edge set is: a second implementation of the
    // grouping would let the audit and the engine disagree about the
    // denominator while appearing to agree about everything else.
    let cycle_partitions = causal_graph.compute_cycle_partitions();

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

    // The series discovery reads for ACTIVITY, which differs from the recorded
    // series only on module-input edges (NaN-shadow repair through the
    // pathway slots). Dumped as `activity_scores` only where it differs, so a
    // consumer derives activity from it and products from `scores` -- exactly
    // the split production makes.
    let activity = simlin_engine::ltm_finding::link_activity_series(
        &results,
        &causal_graph,
        &stocks,
        &link_offsets,
        &sub_model_ports,
    );
    let edges: Vec<serde_json::Value> = link_offsets
        .iter()
        .zip(&activity)
        .map(|(((from, to), offset), act)| {
            let scores: Vec<f64> = (0..results.step_count)
                .map(|s| results.data[s * results.step_size + offset])
                .collect();
            let differs = scores
                .iter()
                .zip(act)
                .any(|(a, b)| a.to_bits() != b.to_bits());
            let mut edge = serde_json::json!({
                "from": from.as_str(),
                "to": to.as_str(),
                "scores": scores_to_json(&scores),
            });
            if differs {
                edge["activity_scores"] = serde_json::Value::Array(scores_to_json(act));
            }
            edge
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
            // `partition` is result-scoped (an index into
            // `DiscoveryResult::partitions`); re-key it onto the full
            // partition list below so the audit and the engine agree on what
            // partition 3 means.
            let partition = l.partition.map(|p| {
                let stocks = &found.partitions[p].stocks;
                cycle_partitions
                    .stock_partition
                    .get(&simlin_engine::common::Ident::new(&stocks[0]))
                    .copied()
                    .expect("a reported partition's stocks are cycle-partition members")
            });
            let scores: Vec<f64> = l.scores.iter().map(|&(_, s)| s).collect();
            serde_json::json!({
                "id": l.loop_info.id,
                "polarity": format!("{:?}", l.loop_info.polarity),
                "avg_abs_score": l.avg_abs_score,
                "nodes": nodes,
                "scores": scores_to_json(&scores),
                "rel_scores": scores_to_json(&l.rel_scores),
                "partition": partition,
            })
        })
        .collect();

    let out = serde_json::json!({
        "model": path,
        "step_count": results.step_count,
        "stocks": stocks.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        "partitions": cycle_partitions
            .partitions
            .iter()
            .map(|p| p.iter().map(|s| s.as_str()).collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        "edges": edges,
        "enumeration_complete": found.enumeration_complete,
        "universe_loops": found.universe_loops,
        "retained_loops": found.retained_loops,
        "truncated": found.truncated,
        "agg_recovery_truncated": found.agg_recovery_truncated,
        // The stitching limits production applies, so an independent
        // re-enumeration stitches under the SAME caps (max petals per
        // aggregate node; stitched-loop budget) rather than re-stating them.
        "max_agg_petals": simlin_engine::ltm_finding::cross_agg_stitching_limits().0,
        "cross_agg_loop_budget": simlin_engine::ltm_finding::cross_agg_stitching_limits().1,
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
