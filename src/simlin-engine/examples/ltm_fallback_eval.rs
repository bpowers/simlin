// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Recall of LTM discovery's shortest-path fallback, measured against the
//! exact union-graph enumeration on real models
//! (docs/design-plans/2026-08-17-ltm-discovery-exact.md, AC7.2).
//!
//! `examples/ltm_discovery_bench` answers "is each strategy fast enough to be
//! a fallback"; this answers "which loops does it lose", which is what settles
//! [`FallbackConfig::DEFAULT`]. For each model it simulates once, runs
//! discovery under `CandidateGen::Auto` (asserting `enumeration_complete`, so
//! the reference really is the exact path), and then re-runs discovery under
//! each strategy over the SAME results.
//!
//! The strategy space has three axes -- the weight formulation, the seed
//! policy, and which cycles a completed search closes -- and they are not
//! independent: widening the seed set and closing on every edge both buy
//! recall with time, and which trade is worth taking is a fact about real
//! runtime graphs rather than something to argue from the shapes.
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
//! Step-dominant coverage is counted per (competing group, saved step) PAIR,
//! not per step. A GLOBAL argmax over `exact.loops` measures nothing: a loop
//! alone in its cycle partition (or the sole retention SURVIVOR of one, once
//! the reported list is capped) has `|rel_scores[t]| == 1` at every active
//! step by construction, so it wins any global argmax it is active for
//! regardless of raw magnitude -- on both models here a global argmax names a
//! non-competing loop at literally every step, which is the design doc's own
//! audit finding restated as a property of THIS harness rather than only of
//! the Python cross-check. A group is "competing" iff at least two REPORTED
//! exact loops share its partition (`DiscoveredPartition::loop_count >= 2` --
//! the same population `rel_scores` is normalized against), matching AC5.1's
//! "within a competing partition". Within a competing group, at each saved
//! step `t in 1..step_count` where some member is active, the pair `(group,
//! t)` is covered iff the fallback reported the group's step-max loop. This is
//! the statistic a dominance-over-time reading depends on: a report that
//! misses the dominant loop at a step names the wrong loop for that step, and
//! counting pairs (rather than requiring every competing group to agree at a
//! step before counting it) keeps one large indifferent partition from hiding
//! a genuine miss in a small competing one.
//!
//! Usage:
//!   cargo run --release --example ltm_fallback_eval [model.mdl ...]

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use salsa::Setter;
use simlin_engine::db::{
    SimlinDb, causal_graph_from_element_edges_with_modules, compile_project_incremental,
    model_element_causal_edges, model_ltm_variables, project_datamodel_dims,
    sync_from_datamodel_incremental,
};
use simlin_engine::ltm_finding::{
    CandidateGen, DiscoveryResult, FallbackClosures, FallbackConfig, FallbackSeeds,
    FallbackTieBreak, FallbackWeight, FoundLoop,
};
use simlin_engine::{canonicalize, open_vensim, open_xmile};

/// Recall is reported at these prefix lengths of the exact ranked list.
const RECALL_KS: [usize; 5] = [1, 10, 50, 100, 200];

/// The strategies measured, with the label used in the printed table.
///
/// Derived by hand from the three enums' variant lists rather than from their
/// full cross product: the weight axis is swept at the cheapest setting of the
/// other two (rows 1-3, which is AC7.2's weight comparison), and the seed and
/// closure axes are then swept at the best weight. A full cross product would
/// be 18 rows of which most repeat the same comparison, and the expensive
/// corners take minutes on World3.
///
/// Adding an enum arm without adding a row here shows up as a missing table
/// row rather than as silently unmeasured behaviour.
const STRATEGIES: [(&str, FallbackConfig); 15] = [
    // The weight axis at the cheapest setting of the other two (AC7.2).
    row(
        "log \\| stock \\| in-edge",
        W_LOG,
        S_STOCK,
        C_IN_EDGE,
        T_HOPS,
    ),
    row(
        "rel \\| stock \\| in-edge",
        W_REL,
        S_STOCK,
        C_IN_EDGE,
        T_HOPS,
    ),
    row(
        "hop \\| stock \\| in-edge",
        W_HOP,
        S_STOCK,
        C_IN_EDGE,
        T_HOPS,
    ),
    row(
        "shift \\| stock \\| in-edge",
        W_SHIFT,
        S_STOCK,
        C_IN_EDGE,
        T_HOPS,
    ),
    // The weight axis again at the closure setting production uses, so the
    // weight verdict is not an artifact of the cheap closure family.
    row(
        "log \\| stock \\| every-edge",
        W_LOG,
        S_STOCK,
        C_EVERY,
        T_HOPS,
    ),
    row(
        "rel \\| stock \\| every-edge",
        W_REL,
        S_STOCK,
        C_EVERY,
        T_HOPS,
    ),
    row(
        "hop \\| stock \\| every-edge",
        W_HOP,
        S_STOCK,
        C_EVERY,
        T_HOPS,
    ),
    row(
        "shift \\| stock \\| every-edge",
        W_SHIFT,
        S_STOCK,
        C_EVERY,
        T_HOPS,
    ),
    // The seed axis at the two contending weights.
    row(
        "log \\| +stockless \\| in-edge",
        W_LOG,
        S_LESS,
        C_IN_EDGE,
        T_HOPS,
    ),
    row(
        "log \\| +stockless \\| every-edge",
        W_LOG,
        S_LESS,
        C_EVERY,
        T_HOPS,
    ),
    row(
        "shift \\| +stockless \\| every-edge",
        W_SHIFT,
        S_LESS,
        C_EVERY,
        T_HOPS,
    ),
    // The tie-break axis, on both contending weights, at the production
    // setting: the question is whether the hop term's bias toward SHORT cycles
    // is what caps recall on a graph whose dominant loops are long.
    row(
        "log \\| +stockless \\| every-edge \\| node-id tie",
        W_LOG,
        S_LESS,
        C_EVERY,
        T_NODE,
    ),
    row(
        "shift \\| +stockless \\| every-edge \\| node-id tie",
        W_SHIFT,
        S_LESS,
        C_EVERY,
        T_NODE,
    ),
    // The widest seed policy, which has to justify its cost.
    row(
        "log \\| all-scc \\| in-edge",
        W_LOG,
        S_ALL,
        C_IN_EDGE,
        T_HOPS,
    ),
    row(
        "log \\| all-scc \\| every-edge",
        W_LOG,
        S_ALL,
        C_EVERY,
        T_HOPS,
    ),
];

const W_LOG: FallbackWeight = FallbackWeight::ClampedLogAbs;
const W_REL: FallbackWeight = FallbackWeight::RelativeLinkScore;
const W_HOP: FallbackWeight = FallbackWeight::HopCount;
const W_SHIFT: FallbackWeight = FallbackWeight::ShiftedLogAbs;
const S_STOCK: FallbackSeeds = FallbackSeeds::Stocks;
const S_LESS: FallbackSeeds = FallbackSeeds::StocksAndStocklessSccs;
const S_ALL: FallbackSeeds = FallbackSeeds::AllSccNodes;
const C_IN_EDGE: FallbackClosures = FallbackClosures::SeedInEdges;
const C_EVERY: FallbackClosures = FallbackClosures::EveryEdge;
const T_HOPS: FallbackTieBreak = FallbackTieBreak::Hops;
const T_NODE: FallbackTieBreak = FallbackTieBreak::NodeId;

/// Spell one strategy row without repeating the field names fifteen times.
/// `\|` escapes the pipe inside a markdown table cell.
const fn row(
    label: &'static str,
    weight: FallbackWeight,
    seeds: FallbackSeeds,
    closures: FallbackClosures,
    tie_break: FallbackTieBreak,
) -> (&'static str, FallbackConfig) {
    (
        label,
        FallbackConfig {
            weight,
            seeds,
            closures,
            tie_break,
        },
    )
}

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

/// Every (competing group, saved step) pair where some group member is
/// active, paired with the index into `exact.loops` of that pair's dominant
/// loop -- the group member with the largest `|rel_scores[t]|` there.
///
/// See the module doc for why this is per (group, step) rather than a global
/// per-step argmax. `rel_scores` is the signed partition-relative series the
/// engine already computed against universe denominators; a NaN or zero entry
/// is "not active at this step" and cannot dominate. Steps are swept over
/// `1..step_count`: step 0 has no `PREVIOUS` value yet, so no link score --
/// and therefore no loop score -- is defined there, matching the range both
/// generators' own sweeps cover.
fn step_dominant_pairs(exact: &DiscoveryResult, step_count: usize) -> Vec<(usize, usize)> {
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, fl) in exact.loops.iter().enumerate() {
        // Competing as the ENGINE classified it: `rel_scores` is normalized
        // against the universe's mass, so a partition whose universe holds
        // several loops leaves a reported loop's |rel| strictly inside (0, 1)
        // at some step even when it is the partition's only reported member
        // (retention or the cap dropped the others); a partition with a
        // one-loop universe pins its loop to exactly +/-1 wherever active and
        // contributes no pairs. Reading the universe off the scores rather
        // than off `DiscoveredPartition::loop_count` (which counts RETURNED
        // loops) keeps a strategy from looking better by missing exactly the
        // steps where a lone survivor of a competing partition dominates.
        let Some(p) = fl.partition else { continue };
        let competing = exact.partitions[p].loop_count >= 2
            || fl
                .rel_scores
                .iter()
                .any(|r| r.is_finite() && r.abs() > 0.0 && r.abs() < 1.0);
        if competing {
            groups.entry(p).or_default().push(i);
        }
    }
    let mut pairs = Vec::new();
    for members in groups.values() {
        for t in 1..step_count {
            let mut best: Option<(usize, f64)> = None;
            for &i in members {
                let Some(&rel) = exact.loops[i].rel_scores.get(t) else {
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
            if let Some((i, _)) = best {
                pairs.push((t, i));
            }
        }
    }
    pairs
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
    // The PRODUCTION constructor (`analysis::analyze_model` calls the
    // identical form): the bare `causal_graph_from_element_edges` leaves
    // module sub-graphs and the variable map empty, silently disabling the
    // discovery-mode per-exit-port pathway recompute (GH #698) on a
    // module-bearing model -- this harness would otherwise measure recall
    // against a DIFFERENT discovery path than `Model.analyze()` runs.
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

    let dominant_pairs = step_dominant_pairs(&exact, results.step_count);

    println!(
        "  saved steps {} | stocks {} | exact run {exact_s:.3}s | exact reported loops {} \
         | competing (group, step) pairs {}",
        results.step_count,
        stocks.len(),
        exact.loops.len(),
        dominant_pairs.len(),
    );
    println!(
        "  distinct dominant loops across those pairs: {}",
        dominant_pairs
            .iter()
            .map(|&(_, i)| i)
            .collect::<HashSet<_>>()
            .len()
    );

    // --- Header: candidate volume, one column per recall K, plus the
    // per-(competing group, step) coverage statistic. ---
    let mut header =
        String::from("| weight \\| seeds \\| closures | time (s) | candidates | loops |");
    let mut rule = String::from("|---|---|---|---|");
    for k in RECALL_KS {
        header.push_str(&format!(" recall@{k} |"));
        rule.push_str("---|");
    }
    header.push_str(" step-dominant (group, step) pairs covered |");
    rule.push_str("---|");
    println!("\n{header}\n{rule}");

    for (label, config) in STRATEGIES {
        let t0 = Instant::now();
        let found = discover(CandidateGen::FallbackOnly(config));
        let elapsed = t0.elapsed().as_secs_f64();
        assert!(
            !found.enumeration_complete,
            "a FallbackOnly run must never claim the enumeration ran"
        );
        let fallback_set: HashSet<Vec<String>> = found.loops.iter().map(loop_key).collect();

        let mut row = format!(
            "| {label} | {elapsed:.3} | {} | {} |",
            found
                .fallback_candidates
                .map_or_else(|| "--".to_string(), |n| n.to_string()),
            found.loops.len()
        );
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

        let covered = dominant_pairs
            .iter()
            .filter(|&&(_, i)| fallback_set.contains(&exact_keys[i]))
            .count();
        let total = dominant_pairs.len();
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
