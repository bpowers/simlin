// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Recall of LTM discovery's shortest-path fallback, measured against the
//! exact union-graph enumeration on real models
//! (docs/design-plans/2026-08-17-ltm-discovery-exact.md, AC7.2).
//!
//! `examples/ltm_discovery_bench` answers "is each weight formulation fast
//! enough to be a fallback"; this answers "which loops does it lose", which is
//! what settles [`FallbackWeight::DEFAULT`]. For each model it simulates once,
//! runs discovery under `CandidateGen::Auto` (asserting
//! `enumeration_complete`, so the reference really is the exact path), and
//! then re-runs discovery under each `FallbackWeight` over the SAME results.
//!
//! **What the numbers establish.** The reference is the `Auto` run's REPORTED
//! loop list: the retention survivors ranked competitive-first by mean
//! relative score and capped at `MAX_LOOPS` (200). So "recall@K" is the
//! fraction of the exact top-K -- by the engine's own ranking statistic,
//! computed against full-universe denominators -- that appears in the
//! fallback's reported list.
//!
//! **What they do NOT establish.** Four things, all of which make these
//! numbers a lower bound on generator recall rather than a measurement of it:
//!
//! 1. The reference is the top-200, not the full retention-survivor set (2,979
//!    on World3). The public API caps what it returns, so a harness built on
//!    public entry points cannot see the survivors it dropped. Recall against
//!    the survivor set is the notebook audit's job, which enumerates in Python.
//! 2. The fallback column is a REPORTED set, not a candidate set. A loop the
//!    fallback proposed can still be lost to its own retention filter or its
//!    own 200-slot cap, so a miss here does not prove the generator never
//!    found the cycle.
//! 3. Both sides' retention denominators come from their own candidate sets:
//!    the enumeration path normalizes against the universe, the fallback path
//!    against whatever it discovered. Two loops with identical raw scores can
//!    therefore be retained on one path and dropped on the other. This is also
//!    why the "share of universe partition mass the fallback's set holds"
//!    statistic is deliberately absent: the two paths' partition totals are
//!    different denominators, so any such ratio would compare incomparable
//!    quantities.
//! 4. Nothing here is a statement about score CORRECTNESS. Both paths score
//!    identically (the per-step product of the recorded link-score series);
//!    only the proposed set differs.
//!
//! Loops are matched by the canonical (lexicographically minimal) rotation of
//! their reported node cycle, so a loop found from a different seed or in a
//! different rotation still matches. Rotation, not sorted node set: two
//! distinct directed cycles over the same nodes are different loops.
//!
//! Step-dominant coverage sweeps saved steps `t in 1..step_count` -- the same
//! range the fallback's own sweep covers, and the range over which link scores
//! are defined (step 0 has no `PREVIOUS` value). At each step with an active
//! loop it takes the exact-path loop with the largest `|rel_scores[t]|` and
//! asks whether the fallback reported it. This is the statistic a
//! dominance-over-time reading depends on: a report that misses the dominant
//! loop at a step names the wrong loop for that step.
//!
//! Usage:
//!   cargo run --release --example ltm_fallback_eval [model.mdl ...]

use std::collections::HashSet;
use std::time::Instant;

use salsa::Setter;
use simlin_engine::db::{
    SimlinDb, causal_graph_from_element_edges, compile_project_incremental,
    model_element_causal_edges, model_ltm_variables, project_datamodel_dims,
    sync_from_datamodel_incremental,
};
use simlin_engine::ltm_finding::{CandidateGen, FallbackWeight, FoundLoop};
use simlin_engine::{canonicalize, open_vensim, open_xmile};

/// Recall is reported at these prefix lengths of the exact ranked list.
const RECALL_KS: [usize; 5] = [1, 10, 50, 100, 200];

/// Every `FallbackWeight` arm, with the label used in the printed table.
///
/// Derived by hand from the enum's variant list rather than from a
/// `strum`-style iterator, so adding an arm without adding it here shows up as
/// a missing table row rather than as silently unmeasured behaviour.
const WEIGHTS: [(&str, FallbackWeight); 3] = [
    ("ClampedLogAbs", FallbackWeight::ClampedLogAbs),
    ("RelativeLinkScore", FallbackWeight::RelativeLinkScore),
    ("HopCount", FallbackWeight::HopCount),
];

/// The lexicographically minimal rotation of a cycle's node sequence.
///
/// A local twin of `crate::ltm::canonical_rotation` (which is `pub(crate)`, so
/// an example cannot call it). Both compute the same thing for the same
/// reason: a cycle has one representation per starting node, and two runs that
/// find the same loop from different seeds must produce the same key. Rotation
/// rather than sorting is load-bearing -- `[a, b, c]` and `[a, c, b]` are
/// distinct directed cycles over the same node set and must not collide.
fn canonical_rotation(cycle: &[String]) -> Vec<String> {
    if cycle.is_empty() {
        return Vec::new();
    }
    let mut best = 0usize;
    for start in 1..cycle.len() {
        let is_smaller = (0..cycle.len()).find_map(|i| {
            let a = &cycle[(start + i) % cycle.len()];
            let b = &cycle[(best + i) % cycle.len()];
            (a != b).then(|| a < b)
        });
        if is_smaller == Some(true) {
            best = start;
        }
    }
    let mut out = Vec::with_capacity(cycle.len());
    out.extend_from_slice(&cycle[best..]);
    out.extend_from_slice(&cycle[..best]);
    out
}

/// A reported loop's identity: the canonical rotation of its node cycle.
///
/// `loop_info.links[i].from` walked in order IS the node cycle (link `i` runs
/// `from -> to` and link `i+1` starts where link `i` ended), which is the same
/// sequence `ltm_finding::loop_sort_key` canonicalizes for id assignment.
fn loop_key(fl: &FoundLoop) -> Vec<String> {
    let nodes: Vec<String> = fl
        .loop_info
        .links
        .iter()
        .map(|l| l.from.as_str().to_string())
        .collect();
    canonical_rotation(&nodes)
}

/// Per-step dominance of the exact path: for each saved step, the index into
/// the exact loop list of the loop with the largest `|rel_scores[t]|`, or
/// `None` where no exact loop is active.
///
/// `rel_scores` is the signed partition-relative series the engine already
/// computed against universe denominators; a NaN or zero entry is "not active
/// at this step" and cannot dominate.
fn step_dominant(exact: &[FoundLoop], step_count: usize) -> Vec<Option<usize>> {
    (0..step_count)
        .map(|t| {
            let mut best: Option<(usize, f64)> = None;
            for (i, fl) in exact.iter().enumerate() {
                let Some(&rel) = fl.rel_scores.get(t) else {
                    continue;
                };
                let mag = rel.abs();
                if mag.is_nan() || mag == 0.0 {
                    continue;
                }
                if best.is_none_or(|(_, b)| mag > b) {
                    best = Some((i, mag));
                }
            }
            best.map(|(i, _)| i)
        })
        .collect()
}

fn main() {
    let mut models: Vec<String> = std::env::args().skip(1).collect();
    if models.is_empty() {
        let base = format!("{}/../../test", env!("CARGO_MANIFEST_DIR"));
        models = vec![
            format!("{base}/metasd/WRLD3-03/wrld3-03.mdl"),
            format!("{base}/xmutil_test_models/C-LEARN v77 for Vensim.mdl"),
        ];
    }

    for path in &models {
        eval_model(path);
    }
}

fn eval_model(path: &str) {
    println!("==> {path}");
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            println!("  skipped (read error: {e})");
            return;
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
    let ports = simlin_engine::analysis::build_sub_model_output_ports(&db, source_project);

    let discover = |generator: CandidateGen| {
        simlin_engine::ltm_finding::discover_loops_with_candidate_gen(
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
        .unwrap()
    };

    // --- Reference: the exact path. ---
    let t0 = Instant::now();
    let exact = discover(CandidateGen::Auto);
    let exact_s = t0.elapsed().as_secs_f64();
    assert!(
        exact.enumeration_complete,
        "the reference run must be the exact enumeration; without it there is \
         nothing to measure recall against"
    );
    let exact_keys: Vec<Vec<String>> = exact.loops.iter().map(loop_key).collect();
    let exact_set: HashSet<&Vec<String>> = exact_keys.iter().collect();
    debug_assert_eq!(
        exact_set.len(),
        exact_keys.len(),
        "the engine deduplicates reported cycles by canonical rotation, so the \
         reference keys must be distinct"
    );

    let dominant = step_dominant(&exact.loops, results.step_count);
    // Step 0 has no link scores (no `PREVIOUS` value yet) and is excluded from
    // both generators' sweeps, so it is excluded here too.
    let dominant_steps: Vec<usize> = (1..results.step_count)
        .filter(|&t| dominant[t].is_some())
        .collect();

    println!(
        "  saved steps {} | stocks {} | exact run {exact_s:.3}s | exact reported loops {} \
         | steps with an active exact loop {} of {}",
        results.step_count,
        stocks.len(),
        exact.loops.len(),
        dominant_steps.len(),
        results.step_count.saturating_sub(1),
    );
    println!(
        "  distinct dominant loops over those steps: {}",
        dominant_steps
            .iter()
            .filter_map(|&t| dominant[t])
            .collect::<HashSet<_>>()
            .len()
    );

    // --- Header: one column per recall K, plus step-dominant coverage. ---
    let mut header = String::from("| weight | time (s) | loops |");
    let mut rule = String::from("|---|---|---|");
    for k in RECALL_KS {
        header.push_str(&format!(" recall@{k} |"));
        rule.push_str("---|");
    }
    header.push_str(" step-dominant covered |");
    rule.push_str("---|");
    println!("\n{header}\n{rule}");

    for (label, weight) in WEIGHTS {
        let t0 = Instant::now();
        let found = discover(CandidateGen::FallbackOnly(weight));
        let elapsed = t0.elapsed().as_secs_f64();
        assert!(
            !found.enumeration_complete,
            "a FallbackOnly run must never claim the enumeration ran"
        );
        let fallback_set: HashSet<Vec<String>> = found.loops.iter().map(loop_key).collect();

        let mut row = format!("| `{label}` | {elapsed:.3} | {} |", found.loops.len());
        for k in RECALL_KS {
            // K is a prefix of the exact ranked list; a model reporting fewer
            // than K exact loops has no top-K, and printing a recall over a
            // shorter list under a "recall@K" heading would overstate it.
            let n = k.min(exact_keys.len());
            if n == 0 {
                row.push_str(" -- |");
                continue;
            }
            let hits = exact_keys[..n]
                .iter()
                .filter(|key| fallback_set.contains(*key))
                .count();
            let note = if n < k {
                format!(" of {n}")
            } else {
                String::new()
            };
            row.push_str(&format!(
                " {:.2} ({hits}/{n}{note}) |",
                hits as f64 / n as f64
            ));
        }

        let covered = dominant_steps
            .iter()
            .filter(|&&t| dominant[t].is_some_and(|i| fallback_set.contains(&exact_keys[i])))
            .count();
        let total = dominant_steps.len();
        row.push_str(&format!(
            " {:.2} ({covered}/{total}) |",
            if total == 0 {
                f64::NAN
            } else {
                covered as f64 / total as f64
            }
        ));
        println!("{row}");
    }
    println!(
        "\n  recall@200 is also the mean over the exact top-200 of \"present in \
         the fallback's report\" -- the same statistic. Every recall@K is \
         bounded above by min(loops, K)/K, since the fallback's own retention \
         filter and cap decide how long its reported list is; a low recall is \
         therefore partly a statement about that list's LENGTH and not only \
         about which cycles the search proposed.\n"
    );
}
