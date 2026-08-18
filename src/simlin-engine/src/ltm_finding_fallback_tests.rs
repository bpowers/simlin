// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Unit tests for `ltm_finding_fallback.rs`, split out of the module body to
//! keep the production file under the per-file line cap (mounted via `#[path]`).

use std::collections::HashMap;

use super::super::{LinkOffset, ScriptedClock, SystemClock};
use super::*;
use crate::common::{Canonical, Ident};
use crate::results::{Method, Specs};

// --- Fixture construction -------------------------------------------------

/// A fixture edge: `(from, to, |link score| at saved steps 1..=n)`.
///
/// The series deliberately starts at saved step **1**: step 0's link scores
/// are NaN by construction (no `PREVIOUS` value exists yet), which is why the
/// sweep skips it, and [`fixture`] materializes that NaN row itself.
type FixtureEdge<'a> = (&'a str, &'a str, Vec<f64>);

/// Build the `IndexedSearch` + `Results` pair the fallback sees in production.
///
/// `parse_link_offsets` hands `IndexedSearch::build` a `(from, to)`-sorted list
/// with one offset per distinct pair, so this helper sorts the fixture the same
/// way before interning: the node ids, the per-node adjacency order, and the
/// stock ids are then exactly the ones production derives for this topology,
/// rather than a hand-chosen numbering that no run produces.
fn fixture(edges: &[FixtureEdge], stocks: &[&str]) -> (IndexedSearch, Results) {
    let mut sorted: Vec<&FixtureEdge> = edges.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0).then_with(|| a.1.cmp(b.1)));
    for w in sorted.windows(2) {
        assert!(
            (w[0].0, w[0].1) != (w[1].0, w[1].1),
            "fixture has a duplicate (from, to) pair; parse_link_offsets emits one offset per pair"
        );
    }

    let live_steps = sorted.first().map(|e| e.2.len()).unwrap_or(0);
    for e in &sorted {
        assert_eq!(
            e.2.len(),
            live_steps,
            "every fixture edge needs a score for every saved step"
        );
    }
    // +1 for the all-NaN step 0.
    let step_count = live_steps + 1;
    let step_size = sorted.len().max(1);

    let mut data = vec![f64::NAN; step_size * step_count];
    let mut link_offsets: Vec<LinkOffset> = Vec::with_capacity(sorted.len());
    for (i, edge) in sorted.iter().enumerate() {
        for (k, value) in edge.2.iter().enumerate() {
            data[(k + 1) * step_size + i] = *value;
        }
        link_offsets.push(((Ident::new(edge.0), Ident::new(edge.1)), i));
    }

    let results = Results {
        offsets: HashMap::new(),
        data: data.into_boxed_slice(),
        step_size,
        step_count,
        specs: Specs {
            start: 0.0,
            stop: live_steps as f64,
            dt: 1.0,
            save_step: 1.0,
            method: Method::Euler,
            n_chunks: 1,
        },
        is_vensim: false,
    };
    let stock_idents: Vec<Ident<Canonical>> = stocks.iter().map(|s| Ident::new(s)).collect();
    (IndexedSearch::build(&link_offsets, &stock_idents), results)
}

/// Node id of `name` within `search`, or a loud failure.
fn node_id(search: &IndexedSearch, name: &str) -> u32 {
    search
        .idents
        .iter()
        .position(|i| i.as_str() == name)
        .unwrap_or_else(|| panic!("fixture has no node named {name}")) as u32
}

/// Render node-id paths as name paths so assertions read as topology.
fn named(search: &IndexedSearch, paths: &[Vec<u32>]) -> Vec<Vec<String>> {
    paths
        .iter()
        .map(|p| {
            p.iter()
                .map(|&n| search.idents[n as usize].as_str().to_string())
                .collect()
        })
        .collect()
}

/// The min-weight elementary cycle through `seed` in the currently loaded
/// step graph, as `(total weight, node path starting at seed)`.
///
/// Composed of exactly the three production calls `sweep` makes per seed, in
/// the same order, so what it pins is the production path rather than a
/// test-only shortcut.
fn shortest_cycle(scratch: &mut FallbackScratch, seed: u32) -> Option<(f64, Vec<u32>)> {
    let completed = scratch.dijkstra_from(seed, None, &mut SystemClock);
    assert!(
        completed,
        "an unbudgeted Dijkstra always runs to completion"
    );
    let mut closings = Vec::new();
    scratch.collect_closings(seed, &mut closings);
    let &(weight, _hops, from) = closings.first()?;
    let mut path = Vec::new();
    scratch.write_forward_path(seed, from, &mut path);
    Some((weight, path))
}

// --- Weight formulations --------------------------------------------------

/// Every [`FallbackWeight`] variant, derived by hand from the enum's arm list.
///
/// The corpora below sweep this rather than a hand-picked subset, so adding an
/// arm without measuring it is a compile-visible omission rather than silently
/// unexercised behaviour.
const WEIGHT_ARMS: [FallbackWeight; 4] = [
    FallbackWeight::ClampedLogAbs,
    FallbackWeight::RelativeLinkScore,
    FallbackWeight::HopCount,
    FallbackWeight::ShiftedLogAbs,
];

/// Spell the step-scoped normalizers positionally for the arm-level tests.
fn norms(in_sum: f64, step_max_finite: f64) -> EdgeNorms {
    EdgeNorms {
        in_sum,
        step_max_finite,
    }
}

#[test]
fn default_weight_is_clamped_log_abs() {
    // The design's starting formulation: the one arm that keeps Dijkstra's
    // non-negativity precondition without needing a per-target normalization.
    assert_eq!(FallbackWeight::DEFAULT, FallbackWeight::ClampedLogAbs);
}

/// The production strategy is the one `examples/ltm_fallback_eval` measured
/// best, and each axis is pinned to the row that settled it.
///
/// The measurement, on World3 and C-LEARN against the exact enumeration:
/// closing on every edge lifts World3's recall of the exact top-200 from 8 to
/// 31 and C-LEARN's from 97 of 153 to 150, at 0.14 s and 0.15 s against a
/// 0.40 s exact World3 run; seeding every node of the cyclic core reaches a
/// little more still and costs 1.03 s on World3, which is more than the exact
/// enumeration it stands in for, so it stays available and unused.
#[test]
fn default_config_is_the_measured_best() {
    assert_eq!(
        FallbackConfig::DEFAULT.weight,
        FallbackWeight::ClampedLogAbs
    );
    assert_eq!(
        FallbackConfig::DEFAULT.seeds,
        FallbackSeeds::StocksAndStocklessSccs
    );
    assert_eq!(
        FallbackConfig::DEFAULT.closures,
        FallbackClosures::EveryEdge
    );
    // `with_weight` moves the weight and nothing else, which is what makes the
    // harness's weight comparison a comparison of weights.
    let rel = FallbackConfig::with_weight(FallbackWeight::RelativeLinkScore);
    assert_eq!(rel.weight, FallbackWeight::RelativeLinkScore);
    assert_eq!(rel.seeds, FallbackConfig::DEFAULT.seeds);
    assert_eq!(rel.closures, FallbackConfig::DEFAULT.closures);
}

#[test]
fn clamped_log_abs_charges_sub_unit_links_and_frees_super_unit_ones() {
    let w = |abs| {
        edge_weight(
            FallbackWeight::ClampedLogAbs,
            abs,
            norms(f64::NAN, f64::NAN),
        )
    };
    // Sub-unit: cost grows as the link weakens.
    assert!((w(0.5) - std::f64::consts::LN_2).abs() < 1e-12);
    assert!(w(0.01) > w(0.5));
    // Unit and super-unit: free (the clamp is what keeps weights non-negative
    // even though a gain > 1 is a "negative" edge in -log space).
    assert_eq!(w(1.0), 0.0);
    assert_eq!(w(2.0), 0.0);
    // Inf is a real divergent signal, and ln(Inf) clamps to a free hop.
    assert_eq!(w(f64::INFINITY), 0.0);
    // The in-edge sum is not consulted by this arm at all.
    assert_eq!(
        edge_weight(FallbackWeight::ClampedLogAbs, 2.0, norms(1.0, 1.0)),
        0.0
    );
}

#[test]
fn hop_count_charges_one_per_hop_regardless_of_score() {
    let w = |abs, sum| edge_weight(FallbackWeight::HopCount, abs, norms(sum, f64::NAN));
    assert_eq!(w(1e-9, 1e-9), 1.0);
    assert_eq!(w(1.0, 2.0), 1.0);
    assert_eq!(w(1e9, 1e9), 1.0);
    assert_eq!(w(f64::INFINITY, f64::INFINITY), 1.0);
}

/// `ShiftedLogAbs` keeps the distinction the clamped arm throws away: among
/// super-unit links a stronger one is CHEAPER, rather than all of them being
/// free.
#[test]
fn shifted_log_abs_ranks_super_unit_links_by_gain() {
    // A step whose strongest active link is 1000.
    let w = |abs| edge_weight(FallbackWeight::ShiftedLogAbs, abs, norms(f64::NAN, 1000.0));
    // The step's strongest link is the free hop; everything else pays the gap.
    assert_eq!(w(1000.0), 0.0);
    assert!((w(100.0) - 10.0f64.ln()).abs() < 1e-12);
    assert!((w(2.0) - 500.0f64.ln()).abs() < 1e-12);
    // Strictly ordered by gain across the whole range, super-unit included --
    // which is exactly what the clamped arm collapses.
    assert!(w(1000.0) < w(100.0));
    assert!(w(100.0) < w(2.0));
    assert!(w(2.0) < w(1.0));
    assert!(w(1.0) < w(0.5));
    let clamped = |abs| edge_weight(FallbackWeight::ClampedLogAbs, abs, norms(f64::NAN, 1000.0));
    assert_eq!(clamped(1000.0), clamped(2.0), "the clamped arm cannot");
    // Never negative: the shift is the step's own maximum, so no active edge
    // can exceed it.
    for abs in [1e-9, 0.5, 1.0, 2.0, 999.999, 1000.0] {
        assert!(w(abs) >= 0.0, "{abs} weighed {}", w(abs));
    }
}

/// The sum over a cycle is `L*ln(max) - ln(product)`, so the arm charges
/// `ln(max)` per hop and credits the cycle's whole gain -- a long high-gain
/// chain can beat a short weak one, which is the trade the clamped arm cannot
/// express.
#[test]
fn shifted_log_abs_trades_cycle_length_against_product() {
    let max = 1000.0f64;
    let w = |abs: f64| edge_weight(FallbackWeight::ShiftedLogAbs, abs, norms(f64::NAN, max));
    // A four-link chain of strong links against a two-link pair of weak ones.
    let long: f64 = [900.0, 800.0, 700.0, 600.0].iter().map(|&a| w(a)).sum();
    let short: f64 = [2.0, 3.0].iter().map(|&a| w(a)).sum();
    assert!(
        long < short,
        "the high-gain four-link cycle must be cheaper: {long} vs {short}"
    );
    // And the identity the doc states, checked rather than asserted in prose.
    let links = [900.0f64, 800.0, 700.0, 600.0];
    let product: f64 = links.iter().product();
    let expected = links.len() as f64 * max.ln() - product.ln();
    assert!((long - expected).abs() < 1e-9, "{long} != {expected}");
}

#[test]
fn shifted_log_abs_inf_arms_keep_dijkstra_well_defined() {
    // An infinite link is the cheapest hop there is, and the shift is taken
    // over the FINITE scores so a finite sibling keeps a finite ordered
    // weight rather than `inf` or NaN.
    let w = |abs, max| edge_weight(FallbackWeight::ShiftedLogAbs, abs, norms(f64::NAN, max));
    assert_eq!(w(f64::INFINITY, 8.0), 0.0);
    assert!((w(2.0, 8.0) - 4.0f64.ln()).abs() < 1e-12);
    // Every active edge infinite: no finite reference exists, and they are all
    // equally divergent, so all are free hops.
    assert_eq!(w(f64::INFINITY, 0.0), 0.0);
}

/// The step shift is the maximum over the step's FINITE active scores, taken
/// through `load_step` rather than hand-supplied -- so what the arm divides by
/// is what production computes.
#[test]
fn the_step_shift_is_the_largest_finite_active_score() {
    let (search, results) = fixture(
        &[
            ("s", "a", vec![4.0, 0.0]),
            ("a", "s", vec![0.25, 9.0]),
            ("s", "b", vec![f64::INFINITY, 1.0]),
            ("b", "s", vec![1.0, 1.0]),
        ],
        &["s"],
    );
    let mut scratch = FallbackScratch::new(&search, FallbackConfig::DEFAULT.tie_break);

    // Step 1: active finite scores are 4.0, 0.25 and 1.0; the infinite one is
    // excluded from the shift and weighs 0 itself.
    scratch.load_step(&search, &results, 1, FallbackWeight::ShiftedLogAbs);
    assert_eq!(scratch.step_max_finite, 4.0);
    let (s, a, b) = (
        node_id(&search, "s"),
        node_id(&search, "a"),
        node_id(&search, "b"),
    );
    let weight_of = |scratch: &FallbackScratch, from: u32, to: u32| -> f64 {
        scratch.adj[from as usize]
            .iter()
            .find(|e| e.node == to)
            .unwrap_or_else(|| panic!("edge {from}->{to} is not active"))
            .weight
    };
    assert_eq!(weight_of(&scratch, s, a), 0.0, "4.0 IS the step maximum");
    assert_eq!(weight_of(&scratch, s, b), 0.0, "an infinite link is free");
    assert!((weight_of(&scratch, a, s) - 16.0f64.ln()).abs() < 1e-12);

    // Step 2: `s->a` is inactive, so the maximum moves to 9.0.
    scratch.load_step(&search, &results, 2, FallbackWeight::ShiftedLogAbs);
    assert_eq!(scratch.step_max_finite, 9.0);
    assert_eq!(weight_of(&scratch, a, s), 0.0);
    assert!((weight_of(&scratch, s, b) - 9.0f64.ln()).abs() < 1e-12);
}

#[test]
fn relative_link_score_normalizes_against_the_targets_in_edges() {
    let w = |abs, sum| edge_weight(FallbackWeight::RelativeLinkScore, abs, norms(sum, f64::NAN));
    // Sole determinant of its target: the relative link score is 1, cost 0.
    assert_eq!(w(4.0, 4.0), 0.0);
    // One of several: -ln of its share (LTM reference 13.3).
    assert!((w(1.0, 4.0) - 4.0f64.ln()).abs() < 1e-12);
    assert!((w(0.5, 14.5) - 29.0f64.ln()).abs() < 1e-12);
    // Always non-negative: a share is at most 1, so -ln(share) >= 0.
    assert!(w(3.0, 3.0000000000000004) >= 0.0);
}

#[test]
fn relative_link_score_inf_arms_keep_dijkstra_well_defined() {
    let w = |abs, sum| edge_weight(FallbackWeight::RelativeLinkScore, abs, norms(sum, f64::NAN));
    // A finite link competing with an Inf sibling has share 0: weight +Inf,
    // so it is never preferred while the Inf sibling is available. It stays
    // TRAVERSABLE (rather than being dropped) so the set of cycles the
    // fallback can reach does not depend on the weight formulation.
    assert_eq!(w(2.0, f64::INFINITY), f64::INFINITY);
    // Inf competing with Inf: the share is NaN, which no ordering can use.
    // Treat it as a free hop -- the pair is equally divergent.
    assert_eq!(w(f64::INFINITY, f64::INFINITY), 0.0);
}

#[test]
fn in_edge_sums_cover_only_the_steps_active_in_edges() {
    // `z` has two in-edges; only one is active at step 1, so the relative
    // link score there normalizes against that one edge alone (share 1,
    // weight 0). At step 2 both are active and the share is 1/4.
    let (search, results) = fixture(
        &[
            ("s", "z", vec![3.0, 1.0]),
            ("y", "z", vec![0.0, 3.0]),
            ("z", "s", vec![1.0, 1.0]),
            ("s", "y", vec![1.0, 1.0]),
        ],
        &["s"],
    );
    let mut scratch = FallbackScratch::new(&search, FallbackConfig::DEFAULT.tie_break);
    let (s, y, z) = (
        node_id(&search, "s"),
        node_id(&search, "y"),
        node_id(&search, "z"),
    );

    scratch.load_step(&search, &results, 1, FallbackWeight::RelativeLinkScore);
    assert_eq!(
        scratch.in_sum[z as usize], 3.0,
        "the 0-score in-edge is out"
    );
    let sz = scratch.adj[s as usize]
        .iter()
        .find(|e| e.node == z)
        .expect("s->z is active at step 1");
    assert_eq!(sz.weight, 0.0);

    scratch.load_step(&search, &results, 2, FallbackWeight::RelativeLinkScore);
    assert_eq!(scratch.in_sum[z as usize], 4.0);
    let sz = scratch.adj[s as usize]
        .iter()
        .find(|e| e.node == z)
        .expect("s->z is active at step 2");
    assert!((sz.weight - 4.0f64.ln()).abs() < 1e-12);
    let yz = scratch.adj[y as usize]
        .iter()
        .find(|e| e.node == z)
        .expect("y->z is active at step 2");
    assert!((yz.weight - (4.0f64 / 3.0).ln()).abs() < 1e-12);
}

// --- Dijkstra exactness ---------------------------------------------------

/// Deterministic PCG-style generator: the exactness corpus must be identical
/// on every machine and every run, so no OS entropy is involved.
struct Lcg(u64);

impl Lcg {
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }

    /// Uniform in [0, 1).
    fn next_unit(&mut self) -> f64 {
        f64::from(self.next_u32()) / f64::from(u32::MAX)
    }
}

/// Brute-force minimum-weight elementary cycle through `seed`, enumerating
/// every simple path over the SAME weighted step graph Dijkstra reads.
///
/// Deliberately unrestricted by SCC, so it also witnesses that Dijkstra's
/// SCC restriction loses no cycle.
fn brute_force_min_cycle(adj: &[Vec<WeightedEdge>], seed: u32) -> Option<f64> {
    fn walk(
        adj: &[Vec<WeightedEdge>],
        seed: u32,
        node: u32,
        on_path: &mut [bool],
        acc: f64,
        best: &mut Option<f64>,
    ) {
        for edge in &adj[node as usize] {
            if edge.node == seed {
                let total = acc + edge.weight;
                if best.is_none_or(|b| total < b) {
                    *best = Some(total);
                }
            } else if !on_path[edge.node as usize] {
                on_path[edge.node as usize] = true;
                walk(adj, seed, edge.node, on_path, acc + edge.weight, best);
                on_path[edge.node as usize] = false;
            }
        }
    }
    let mut on_path = vec![false; adj.len()];
    on_path[seed as usize] = true;
    let mut best = None;
    walk(adj, seed, seed, &mut on_path, 0.0, &mut best);
    best
}

/// Sum a path's edge weights, closing edge included, straight from the loaded
/// step graph -- an independent recomputation of what Dijkstra reported.
fn path_weight(adj: &[Vec<WeightedEdge>], path: &[u32]) -> f64 {
    let mut total = 0.0;
    for i in 0..path.len() {
        let from = path[i];
        let to = path[(i + 1) % path.len()];
        let edge = adj[from as usize]
            .iter()
            .find(|e| e.node == to)
            .unwrap_or_else(|| {
                panic!("emitted path uses an edge that is not active: {from}->{to}")
            });
        total += edge.weight;
    }
    total
}

/// AC2.3: Dijkstra is exact for non-negative weights. Over a deterministic
/// corpus of small random graphs, the minimum-weight cycle through the seed
/// equals a brute-force enumeration's minimum -- for every weight arm, since
/// each arm produces a different (still non-negative) weighting and the
/// exactness claim is about the search, not the formulation.
#[test]
fn dijkstra_min_cycle_matches_brute_force_on_random_graphs() {
    let arms = WEIGHT_ARMS;
    let names = ["n0", "n1", "n2", "n3", "n4", "n5", "n6", "n7"];
    let mut compared_with_cycle = 0usize;
    let mut compared_without_cycle = 0usize;

    for seed_value in 1..=40u64 {
        let mut rng = Lcg(seed_value);
        let n = 3 + (rng.next_u32() % 6) as usize; // 3..=8 nodes
        // Sweep the density so the corpus covers both regimes: sparse graphs
        // where the seed often sits on no cycle at all (the arm where the
        // search must report nothing) and dense ones with many competing
        // cycles through it.
        let density = 0.10 + 0.06 * ((seed_value % 6) as f64);
        let mut edges: Vec<FixtureEdge> = Vec::new();
        for from in 0..n {
            for to in 0..n {
                if from == to || rng.next_unit() > density {
                    continue;
                }
                // exp(uniform(-3, 3)): a mix of sub- and super-unit links, so
                // the clamped arm sees both free and costly hops.
                let score = (rng.next_unit() * 6.0 - 3.0).exp();
                edges.push((names[from], names[to], vec![score]));
            }
        }
        if edges.is_empty() {
            continue;
        }
        let (search, results) = fixture(&edges, &["n0"]);
        let seed = node_id(&search, "n0");
        let mut scratch = FallbackScratch::new(&search, FallbackConfig::DEFAULT.tie_break);

        for arm in arms {
            scratch.load_step(&search, &results, 1, arm);
            let expected = brute_force_min_cycle(&scratch.adj, seed);
            let found = shortest_cycle(&mut scratch, seed);
            match (expected, found) {
                (None, None) => compared_without_cycle += 1,
                (Some(expected), Some((weight, path))) => {
                    compared_with_cycle += 1;
                    assert!(
                        (weight - expected).abs() <= 1e-9 * expected.abs().max(1.0),
                        "graph {seed_value} arm {arm:?}: Dijkstra reported {weight}, \
                         brute force {expected}"
                    );
                    // The reported weight really is the emitted path's weight.
                    let recomputed = path_weight(&scratch.adj, &path);
                    assert!(
                        (recomputed - weight).abs() <= 1e-9 * weight.abs().max(1.0),
                        "graph {seed_value} arm {arm:?}: path weight {recomputed} != {weight}"
                    );
                    assert_eq!(path[0], seed, "the cycle starts at the seed");
                }
                (expected, found) => panic!(
                    "graph {seed_value} arm {arm:?}: brute force {expected:?} vs Dijkstra {found:?}"
                ),
            }
        }
    }

    // Guard against a corpus that silently degenerates into "no graph has a
    // cycle" (which would make the comparison vacuous) or the reverse.
    assert!(
        compared_with_cycle >= 60,
        "corpus must exercise the cycle-found arm ({compared_with_cycle} cases)"
    );
    assert!(
        compared_without_cycle >= 10,
        "corpus must exercise the no-cycle arm ({compared_without_cycle} cases)"
    );
}

// --- Weight arms drive candidate selection --------------------------------

/// The in-edge closure family under one named weight -- the configuration the
/// three weight-preference tests below pin.
///
/// Explicit rather than `with_weight`, because those tests are about the ORDER
/// a single shortest-path tree ranks its closures in, and the every-edge
/// family (which the production default carries) emits by closing-edge source
/// instead. Both orders are content-pure; only this one expresses the weight.
fn in_edge_closures(weight: FallbackWeight) -> FallbackConfig {
    FallbackConfig {
        weight,
        seeds: FallbackSeeds::Stocks,
        closures: FallbackClosures::SeedInEdges,
        tie_break: FallbackConfig::DEFAULT.tie_break,
    }
}

/// Three cycles through one stock, arranged so that each weight formulation
/// picks a different one:
///
/// * `s -> a -> s`  -- shortest, all sub-unit links
/// * `s -> b -> c -> s` -- all super-unit links (free under the clamped arm)
/// * `s -> d -> e -> s` -- the largest share of `s`'s in-edge mass
fn three_way_fixture() -> (IndexedSearch, Results) {
    fixture(
        &[
            ("s", "a", vec![0.5]),
            ("a", "s", vec![0.5]),
            ("s", "b", vec![2.0]),
            ("b", "c", vec![3.0]),
            ("c", "s", vec![4.0]),
            ("s", "d", vec![0.9]),
            ("d", "e", vec![1.0]),
            ("e", "s", vec![10.0]),
        ],
        &["s"],
    )
}

#[test]
fn clamped_log_abs_prefers_the_super_unit_path() {
    let (search, results) = three_way_fixture();
    let out = sweep(
        &search,
        &results,
        in_edge_closures(FallbackWeight::ClampedLogAbs),
        None,
        &mut SystemClock,
    );
    // Every cycle through the stock is proposed; the minimum comes first.
    assert_eq!(
        named(&search, &out.paths),
        vec![vec!["s", "b", "c"], vec!["s", "d", "e"], vec!["s", "a"],],
    );
}

#[test]
fn hop_count_prefers_the_shortest_path() {
    let (search, results) = three_way_fixture();
    let out = sweep(
        &search,
        &results,
        in_edge_closures(FallbackWeight::HopCount),
        None,
        &mut SystemClock,
    );
    assert_eq!(
        named(&search, &out.paths),
        vec![vec!["s", "a"], vec!["s", "b", "c"], vec!["s", "d", "e"],],
    );
}

#[test]
fn relative_link_score_prefers_the_largest_share_of_the_stocks_in_edges() {
    let (search, results) = three_way_fixture();
    let out = sweep(
        &search,
        &results,
        in_edge_closures(FallbackWeight::RelativeLinkScore),
        None,
        &mut SystemClock,
    );
    assert_eq!(
        named(&search, &out.paths),
        vec![vec!["s", "d", "e"], vec!["s", "b", "c"], vec!["s", "a"],],
    );

    // Hand-computed normalization: `s` has in-edges a->s (0.5), c->s (4.0)
    // and e->s (10.0), so its determinant mass is 14.5. Every other node has
    // a single determinant, so all non-closing hops are free and each cycle's
    // weight is just -ln(share) of its closing edge.
    let mut scratch = FallbackScratch::new(&search, FallbackConfig::DEFAULT.tie_break);
    scratch.load_step(&search, &results, 1, FallbackWeight::RelativeLinkScore);
    let s = node_id(&search, "s");
    assert_eq!(scratch.in_sum[s as usize], 14.5);
    let completed = scratch.dijkstra_from(s, None, &mut SystemClock);
    assert!(completed);
    let mut closings = Vec::new();
    scratch.collect_closings(s, &mut closings);
    let weights: Vec<f64> = closings.iter().map(|&(w, _, _)| w).collect();
    let expected = [
        (14.5f64 / 10.0).ln(),
        (14.5f64 / 4.0).ln(),
        (14.5f64 / 0.5).ln(),
    ];
    assert_eq!(weights.len(), expected.len());
    for (got, want) in weights.iter().zip(expected.iter()) {
        assert!((got - want).abs() < 1e-12, "{got} != {want}");
    }
}

// --- Emitted cycles are well formed ---------------------------------------

/// AC2.3: every emitted cycle is elementary, closes through a seed stock, and
/// is simultaneously active at some saved step -- never assembled from links
/// that are zero or NaN when they were traversed.
#[test]
fn emitted_cycles_are_elementary_and_simultaneously_active() {
    // Every arm of the activity rule is represented by a cycle through the
    // stock, so what the sweep emits pins the rule rather than incidentally
    // agreeing with it:
    //   s<->a  finite nonzero at steps 1 and 3   -> found
    //   s<->b  the return link is NaN at step 2  -> found only at step 3
    //   s<->c  finite nonzero at steps 1 and 2   -> found
    //   s<->d  the return link is always NaN     -> never found
    //   s<->e  the return link is always zero    -> never found
    //   s<->f  the outbound link is always Inf   -> found (Inf is signal)
    let edges: Vec<FixtureEdge> = vec![
        ("s", "a", vec![1.0, 0.0, 1.0]),
        ("a", "s", vec![1.0, 0.0, 1.0]),
        ("s", "b", vec![0.0, 2.0, 2.0]),
        ("b", "s", vec![0.0, f64::NAN, 2.0]),
        ("s", "c", vec![1.0, 1.0, 0.0]),
        ("c", "s", vec![1.0, 1.0, 0.0]),
        ("s", "d", vec![1.0, 1.0, 1.0]),
        ("d", "s", vec![f64::NAN, f64::NAN, f64::NAN]),
        ("s", "e", vec![1.0, 1.0, 1.0]),
        ("e", "s", vec![0.0, 0.0, 0.0]),
        ("s", "f", vec![f64::INFINITY, f64::INFINITY, f64::INFINITY]),
        ("f", "s", vec![1.0, 1.0, 1.0]),
    ];
    let (search, results) = fixture(&edges, &["s"]);
    let out = sweep(
        &search,
        &results,
        FallbackConfig::DEFAULT,
        None,
        &mut SystemClock,
    );
    assert!(!out.truncated);
    assert_eq!(out.steps_processed, 3);

    let by_pair: HashMap<(&str, &str), &Vec<f64>> =
        edges.iter().map(|e| ((e.0, e.1), &e.2)).collect();
    let paths = named(&search, &out.paths);
    let mut node_sets: Vec<Vec<String>> = paths.clone();
    for path in node_sets.iter_mut() {
        path.sort();
    }
    node_sets.sort();
    assert_eq!(
        node_sets,
        vec![
            vec!["a".to_string(), "s".to_string()],
            vec!["b".to_string(), "s".to_string()],
            vec!["c".to_string(), "s".to_string()],
            vec!["f".to_string(), "s".to_string()],
        ],
        "a link that is always NaN or always zero closes no cycle; an infinite one does"
    );

    for path in &paths {
        let mut seen: Vec<&String> = path.iter().collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), path.len(), "{path:?} repeats a node");
        assert!(path.contains(&"s".to_string()), "{path:?} misses the stock");

        // Some ONE saved step has every edge of the cycle active -- the step
        // it was found at. A cycle stitched together from links that are only
        // active at different steps would pass a per-edge check and fail here.
        let live = (0..3).any(|step| {
            (0..path.len()).all(|i| {
                let from = path[i].as_str();
                let to = path[(i + 1) % path.len()].as_str();
                let v = by_pair[&(from, to)][step];
                v.is_infinite() || (v != 0.0 && v.is_finite())
            })
        });
        assert!(live, "{path:?} is never simultaneously active");
    }
}

// --- Search order: the hop tie-break --------------------------------------

/// B1: among equally-weighted cycles the SHORTER one is preferred.
///
/// Under `ClampedLogAbs` every super-unit link weighs exactly 0, so a graph of
/// them ties every route at weight 0 and the weight alone decides nothing --
/// on World3 roughly a third of the active edges are in that plateau. The
/// fixture makes node-id order and hop order disagree deliberately: the THREE
/// node cycle closes through `ab` (node id 1) and the TWO node cycle through
/// `zz` (node id 3), so a search ordered on `(weight, node)` would rank the
/// long cycle first and one ordered on `(weight, hops, node)` ranks the short
/// one first.
#[test]
fn equal_weight_cycles_are_ordered_by_hop_count_not_node_id() {
    let (search, results) = fixture(
        &[
            ("aa", "ab", vec![2.0]),
            ("ab", "s", vec![2.0]),
            ("s", "aa", vec![2.0]),
            ("s", "zz", vec![2.0]),
            ("zz", "s", vec![2.0]),
        ],
        &["s"],
    );
    let (s, ab, zz) = (
        node_id(&search, "s"),
        node_id(&search, "ab"),
        node_id(&search, "zz"),
    );
    assert!(
        ab < zz,
        "the long cycle must close through the lower node id"
    );

    let mut scratch = FallbackScratch::new(&search, FallbackConfig::DEFAULT.tie_break);
    scratch.load_step(&search, &results, 1, FallbackWeight::ClampedLogAbs);
    assert!(scratch.dijkstra_from(s, None, &mut SystemClock));
    let mut closings = Vec::new();
    scratch.collect_closings(s, &mut closings);
    assert_eq!(
        closings,
        vec![(0.0, 2, zz), (0.0, 3, ab)],
        "both cycles weigh 0; the two-hop one comes first"
    );

    let out = sweep(
        &search,
        &results,
        in_edge_closures(FallbackWeight::ClampedLogAbs),
        None,
        &mut SystemClock,
    );
    assert_eq!(
        named(&search, &out.paths),
        vec![vec!["s", "zz"], vec!["s", "aa", "ab"]],
        "the sweep emits a seed's closures cheapest first, and among equals \
         shortest first"
    );
}

/// Both [`FallbackTieBreak`] arms, on the fixture where they disagree: the
/// three-node cycle closes through the LOWER node id, so ordering on node id
/// alone ranks it first while ordering on hops first ranks the two-node cycle.
///
/// The control matters because it is what the hop term is measured against in
/// `examples/ltm_fallback_eval.rs`: without an arm that demonstrably picks
/// differently, "the hop tie-break changes nothing" would be unfalsifiable.
#[test]
fn each_tie_break_arm_orders_equal_weight_cycles_its_own_way() {
    let (search, results) = fixture(
        &[
            ("aa", "ab", vec![2.0]),
            ("ab", "s", vec![2.0]),
            ("s", "aa", vec![2.0]),
            ("s", "zz", vec![2.0]),
            ("zz", "s", vec![2.0]),
        ],
        &["s"],
    );
    let order = |tie_break| -> Vec<Vec<String>> {
        let out = sweep(
            &search,
            &results,
            FallbackConfig {
                weight: FallbackWeight::ClampedLogAbs,
                seeds: FallbackSeeds::Stocks,
                closures: FallbackClosures::SeedInEdges,
                tie_break,
            },
            None,
            &mut SystemClock,
        );
        named(&search, &out.paths)
    };
    assert_eq!(
        order(FallbackTieBreak::Hops),
        vec![vec!["s", "zz"], vec!["s", "aa", "ab"]],
        "shortest first among equal weights"
    );
    assert_eq!(
        order(FallbackTieBreak::NodeId),
        vec![vec!["s", "aa", "ab"], vec!["s", "zz"]],
        "lowest closing node id first, which is the longer cycle here"
    );
}

/// Turning the hop term off changes the ORDER and nothing else: the same
/// cycles are found either way, since the tie-break decides which of two
/// equally-weighted routes a tree holds, not which cycles exist.
#[test]
fn the_tie_break_does_not_change_which_cycles_exist() {
    let names = ["n0", "n1", "n2", "n3", "n4", "n5", "n6", "n7"];
    let mut compared = 0usize;
    for seed_value in 1..=40u64 {
        let mut rng = Lcg(seed_value);
        let n = 3 + (rng.next_u32() % 6) as usize;
        let density = 0.10 + 0.06 * ((seed_value % 6) as f64);
        let mut edges: Vec<FixtureEdge> = Vec::new();
        for from in 0..n {
            for to in 0..n {
                if from == to || rng.next_unit() > density {
                    continue;
                }
                let score = (rng.next_unit() * 6.0 - 3.0).exp();
                edges.push((names[from], names[to], vec![score]));
            }
        }
        if edges.is_empty() {
            continue;
        }
        let (search, results) = fixture(&edges, &["n0"]);
        let run = |tie_break| {
            sweep(
                &search,
                &results,
                FallbackConfig {
                    tie_break,
                    ..FallbackConfig::DEFAULT
                },
                None,
                &mut SystemClock,
            )
        };
        let hops = run(FallbackTieBreak::Hops);
        let node = run(FallbackTieBreak::NodeId);
        for path in &hops.paths {
            assert!(
                node.paths.iter().any(|p| is_same_cycle(path, p)),
                "graph {seed_value}: the node-id arm lost {path:?}"
            );
            compared += 1;
        }
        assert_eq!(
            hops.paths.len(),
            node.paths.len(),
            "graph {seed_value}: the two arms found different counts"
        );
    }
    assert!(compared >= 20, "corpus must find cycles ({compared})");
}

// --- Every-edge closures --------------------------------------------------

/// Every simple cycle through `seed`, as `(total weight, node path starting at
/// the seed)`.
///
/// The oracle the every-edge closures are measured against: it enumerates
/// simple paths directly over the same weighted step graph the search reads,
/// sharing no code with the search itself.
fn brute_force_cycles_through_seed(adj: &[Vec<WeightedEdge>], seed: u32) -> Vec<(f64, Vec<u32>)> {
    fn walk(
        adj: &[Vec<WeightedEdge>],
        seed: u32,
        node: u32,
        on_path: &mut [bool],
        path: &mut Vec<u32>,
        acc: f64,
        out: &mut Vec<(f64, Vec<u32>)>,
    ) {
        for edge in &adj[node as usize] {
            if edge.node == seed {
                out.push((acc + edge.weight, path.clone()));
            } else if !on_path[edge.node as usize] {
                on_path[edge.node as usize] = true;
                path.push(edge.node);
                walk(adj, seed, edge.node, on_path, path, acc + edge.weight, out);
                path.pop();
                on_path[edge.node as usize] = false;
            }
        }
    }
    let mut on_path = vec![false; adj.len()];
    on_path[seed as usize] = true;
    let mut path = vec![seed];
    let mut out = Vec::new();
    walk(adj, seed, seed, &mut on_path, &mut path, 0.0, &mut out);
    out
}

/// Whether `cycle` (a node sequence, closing implicitly) traverses `from -> to`.
fn cycle_has_edge(cycle: &[u32], from: u32, to: u32) -> bool {
    (0..cycle.len()).any(|i| cycle[i] == from && cycle[(i + 1) % cycle.len()] == to)
}

/// The closures produced for one (seed, step) under `EveryEdge`, as node paths.
fn every_edge_closures(scratch: &mut FallbackScratch, seed: u32) -> Vec<Vec<u32>> {
    assert!(scratch.dijkstra_from(seed, None, &mut SystemClock));
    assert!(scratch.dijkstra_to(seed, None, &mut SystemClock));
    let mut dedup = CycleDedup::default();
    let mut paths = Vec::new();
    let mut cycle = Vec::new();
    scratch.collect_every_edge_closures(seed, &mut cycle, &mut dedup, &mut paths);
    paths
}

/// AC2.3 for the every-edge family, against the brute-force oracle on the same
/// deterministic corpus the single-tree exactness test uses.
///
/// Two claims, and the first is the one that says what this closure family IS:
/// every emitted cycle is, for at least one of its own edges, the
/// minimum-weight simple cycle through the seed and that edge. That holds
/// unconditionally -- a closure's weight is the sum of two shortest-path
/// halves, which lower-bounds every simple cycle through the seed and its
/// edge, and an emitted closure is itself such a cycle, so the bound is met.
///
/// The second is coverage: over the corpus most (edge, seed) pairs that have a
/// simple cycle at all get one at the oracle's minimum. It is not all of them,
/// and that is the family's stated boundary rather than a defect: when the two
/// tree halves share a node the concatenation is not elementary and the
/// candidate is skipped.
#[test]
fn every_edge_closures_are_minimum_weight_cycles_through_their_edge() {
    let arms = WEIGHT_ARMS;
    let names = ["n0", "n1", "n2", "n3", "n4", "n5", "n6", "n7"];
    let mut checked_cycles = 0usize;
    let mut pairs_with_a_cycle = 0usize;
    let mut pairs_covered = 0usize;

    for seed_value in 1..=40u64 {
        let mut rng = Lcg(seed_value);
        let n = 3 + (rng.next_u32() % 6) as usize;
        let density = 0.10 + 0.06 * ((seed_value % 6) as f64);
        let mut edges: Vec<FixtureEdge> = Vec::new();
        for from in 0..n {
            for to in 0..n {
                if from == to || rng.next_unit() > density {
                    continue;
                }
                let score = (rng.next_unit() * 6.0 - 3.0).exp();
                edges.push((names[from], names[to], vec![score]));
            }
        }
        if edges.is_empty() {
            continue;
        }
        let (search, results) = fixture(&edges, &["n0"]);
        let seed = node_id(&search, "n0");
        let mut scratch = FallbackScratch::new(&search, FallbackConfig::DEFAULT.tie_break);

        for arm in arms {
            scratch.load_step(&search, &results, 1, arm);
            let oracle = brute_force_cycles_through_seed(&scratch.adj, seed);
            let emitted = every_edge_closures(&mut scratch, seed);

            for cycle in &emitted {
                // Soundness: elementary, through the seed, over active edges.
                let mut seen: Vec<u32> = cycle.clone();
                seen.sort_unstable();
                seen.dedup();
                assert_eq!(seen.len(), cycle.len(), "{cycle:?} repeats a node");
                assert!(cycle.contains(&seed), "{cycle:?} misses the seed");
                let weight = path_weight(&scratch.adj, cycle);

                // Optimality: minimal for at least one of its own edges.
                let optimal_for_some_edge = (0..cycle.len()).any(|i| {
                    let (u, w) = (cycle[i], cycle[(i + 1) % cycle.len()]);
                    let best = oracle
                        .iter()
                        .filter(|(_, c)| cycle_has_edge(c, u, w))
                        .map(|(weight, _)| *weight)
                        .fold(f64::INFINITY, f64::min);
                    (weight - best).abs() <= 1e-9 * best.abs().max(1.0)
                });
                assert!(
                    optimal_for_some_edge,
                    "graph {seed_value} arm {arm:?}: {cycle:?} at {weight} is not the \
                     minimum-weight cycle through the seed and any of its own edges"
                );
                checked_cycles += 1;
            }

            // Coverage: how many (edge, seed) pairs the family reaches.
            for u in 0..scratch.adj.len() as u32 {
                for k in 0..scratch.adj[u as usize].len() {
                    let w = scratch.adj[u as usize][k].node;
                    let best = oracle
                        .iter()
                        .filter(|(_, c)| cycle_has_edge(c, u, w))
                        .map(|(weight, _)| *weight)
                        .fold(f64::INFINITY, f64::min);
                    if !best.is_finite() {
                        continue;
                    }
                    pairs_with_a_cycle += 1;
                    let covered = emitted.iter().any(|c| {
                        cycle_has_edge(c, u, w)
                            && (path_weight(&scratch.adj, c) - best).abs()
                                <= 1e-9 * best.abs().max(1.0)
                    });
                    if covered {
                        pairs_covered += 1;
                    }
                }
            }
        }
    }

    assert!(
        checked_cycles >= 100,
        "corpus must actually emit closures ({checked_cycles} checked)"
    );
    assert!(
        pairs_with_a_cycle >= 100,
        "corpus must contain edges on cycles ({pairs_with_a_cycle})"
    );
    // The family is a strong sampler, not an enumeration: a closure whose two
    // tree halves overlap is skipped, so coverage is high rather than total.
    assert!(
        pairs_covered * 4 >= pairs_with_a_cycle * 3,
        "every-edge closures should cover most edge-and-seed pairs, covered \
         {pairs_covered} of {pairs_with_a_cycle}"
    );
}

/// The every-edge family is a SUPERSET of the seed-in-edge closures: the
/// `w == seed` cases are exactly those, so nothing the cheap policy finds can
/// be lost by turning the expensive one on.
#[test]
fn every_edge_closures_include_every_seed_in_edge_closure() {
    let names = ["n0", "n1", "n2", "n3", "n4", "n5", "n6", "n7"];
    let mut compared = 0usize;
    for seed_value in 1..=40u64 {
        let mut rng = Lcg(seed_value);
        let n = 3 + (rng.next_u32() % 6) as usize;
        let density = 0.10 + 0.06 * ((seed_value % 6) as f64);
        let mut edges: Vec<FixtureEdge> = Vec::new();
        for from in 0..n {
            for to in 0..n {
                if from == to || rng.next_unit() > density {
                    continue;
                }
                let score = (rng.next_unit() * 6.0 - 3.0).exp();
                edges.push((names[from], names[to], vec![score]));
            }
        }
        if edges.is_empty() {
            continue;
        }
        let (search, results) = fixture(&edges, &["n0"]);
        let cheap = sweep(
            &search,
            &results,
            FallbackConfig {
                closures: FallbackClosures::SeedInEdges,
                ..FallbackConfig::DEFAULT
            },
            None,
            &mut SystemClock,
        );
        let rich = sweep(
            &search,
            &results,
            FallbackConfig {
                closures: FallbackClosures::EveryEdge,
                ..FallbackConfig::DEFAULT
            },
            None,
            &mut SystemClock,
        );
        for path in &cheap.paths {
            assert!(
                rich.paths.iter().any(|p| is_same_cycle(path, p)),
                "graph {seed_value}: every-edge closures dropped {path:?}"
            );
            compared += 1;
        }
    }
    assert!(compared >= 20, "corpus must find cycles ({compared})");
}

/// AC2.4's diamond, re-measured: two parallel routes between the same pair of
/// nodes are exactly what ONE shortest-path tree cannot express, and exactly
/// what closing on every edge recovers -- the edge `y -> z` is closed by the
/// tree path to `y` and the reverse tree path from `z`.
#[test]
fn every_edge_closures_recover_both_arms_of_a_diamond() {
    let (search, results) = fixture(
        &[
            ("s", "x", vec![2.0]),
            ("s", "y", vec![2.0]),
            ("x", "z", vec![2.0]),
            ("y", "z", vec![2.0]),
            ("z", "s", vec![2.0]),
        ],
        &["s"],
    );
    let sets = |out: &FallbackOutcome| -> Vec<Vec<String>> {
        let mut sets: Vec<Vec<String>> = named(&search, &out.paths)
            .into_iter()
            .map(|mut p| {
                p.sort();
                p
            })
            .collect();
        sets.sort();
        sets
    };

    let cheap = sweep(
        &search,
        &results,
        FallbackConfig {
            closures: FallbackClosures::SeedInEdges,
            ..FallbackConfig::DEFAULT
        },
        None,
        &mut SystemClock,
    );
    assert_eq!(
        cheap.paths.len(),
        1,
        "one tree holds one route to `z`, so the seed's single in-edge closes \
         one arm: {:?}",
        named(&search, &cheap.paths)
    );

    let rich = sweep(
        &search,
        &results,
        FallbackConfig {
            closures: FallbackClosures::EveryEdge,
            ..FallbackConfig::DEFAULT
        },
        None,
        &mut SystemClock,
    );
    assert_eq!(
        sets(&rich),
        vec![
            vec!["s".to_string(), "x".to_string(), "z".to_string()],
            vec!["s".to_string(), "y".to_string(), "z".to_string()],
        ],
    );
}

// --- Seed policy ----------------------------------------------------------

/// A stock cycle beside a cycle with no stock in it -- the shape of
/// module-level state or a `PREVIOUS` lag between two auxes.
fn stock_and_stockless_cycles() -> (IndexedSearch, Results) {
    fixture(
        &[
            ("s", "f", vec![2.0]),
            ("f", "s", vec![2.0]),
            ("p", "q", vec![2.0]),
            ("q", "p", vec![2.0]),
        ],
        &["s"],
    )
}

/// Each seed policy is pinned on the one thing that separates it from its
/// neighbour: whether a cycle touching no stock is reachable at all.
#[test]
fn the_seed_policy_decides_whether_a_stockless_cycle_is_reachable() {
    let (search, results) = stock_and_stockless_cycles();
    let stock_cycle = vec!["f".to_string(), "s".to_string()];
    let stockless_cycle = vec!["p".to_string(), "q".to_string()];
    let sets = |policy: FallbackSeeds| -> Vec<Vec<String>> {
        let out = sweep(
            &search,
            &results,
            FallbackConfig {
                seeds: policy,
                ..FallbackConfig::DEFAULT
            },
            None,
            &mut SystemClock,
        );
        let mut sets: Vec<Vec<String>> = named(&search, &out.paths)
            .into_iter()
            .map(|mut p| {
                p.sort();
                p
            })
            .collect();
        sets.sort();
        sets
    };

    assert_eq!(
        sets(FallbackSeeds::Stocks),
        vec![stock_cycle.clone()],
        "stock seeds reach no cycle that holds no stock"
    );
    assert_eq!(
        sets(FallbackSeeds::StocksAndStocklessSccs),
        vec![stock_cycle.clone(), stockless_cycle.clone()],
        "one representative per stockless component closes that gap"
    );
    assert_eq!(
        sets(FallbackSeeds::AllSccNodes),
        vec![stock_cycle, stockless_cycle],
        "seeding every node in the cyclic core reaches both"
    );
}

/// The seed sets themselves, so the policies are pinned on WHICH nodes they
/// search from and not only on what those searches happen to find. A node on
/// no cycle at this step is never a seed under any policy -- a search from one
/// can close nothing.
#[test]
fn each_seed_policy_selects_the_nodes_it_names() {
    // `dead` sits outside every cycle, so no policy may seed it.
    let (search, results) = fixture(
        &[
            ("s", "f", vec![2.0]),
            ("f", "s", vec![2.0]),
            ("p", "q", vec![2.0]),
            ("q", "p", vec![2.0]),
            ("s", "dead", vec![2.0]),
        ],
        &["s"],
    );
    let mut scratch = FallbackScratch::new(&search, FallbackConfig::DEFAULT.tie_break);
    scratch.load_step(&search, &results, 1, FallbackWeight::DEFAULT);
    let names_of = |scratch: &mut FallbackScratch, policy| -> Vec<String> {
        let mut seeds = Vec::new();
        scratch.collect_seeds(&search, policy, &mut seeds);
        seeds
            .iter()
            .map(|&n| search.idents[n as usize].as_str().to_string())
            .collect()
    };

    assert_eq!(names_of(&mut scratch, FallbackSeeds::Stocks), vec!["s"]);
    // `p` is the lower-id node of the stockless component, so it represents it.
    assert_eq!(
        names_of(&mut scratch, FallbackSeeds::StocksAndStocklessSccs),
        vec!["s", "p"],
        "stocks first, then one representative per stockless component"
    );
    let mut all = names_of(&mut scratch, FallbackSeeds::AllSccNodes);
    all.sort();
    assert_eq!(
        all,
        vec!["f", "p", "q", "s"],
        "every node in a non-trivial component, and nothing outside one"
    );
}

// --- Self edges -----------------------------------------------------------

#[test]
fn a_stock_with_only_a_self_edge_yields_no_cycle() {
    // A one-variable "loop" is not a feedback loop in the SD sense, and the
    // enumerator does not emit one either -- the two generators must agree on
    // what a loop is.
    let (search, results) = fixture(&[("s", "s", vec![2.0])], &["s"]);
    let out = sweep(
        &search,
        &results,
        FallbackConfig::DEFAULT,
        None,
        &mut SystemClock,
    );
    assert!(out.paths.is_empty());
    assert!(!out.truncated);
}

#[test]
fn a_self_edge_inside_a_real_cycle_is_never_traversed() {
    let (search, results) = fixture(
        &[
            ("s", "a", vec![1.0]),
            ("a", "s", vec![1.0]),
            ("a", "a", vec![5.0]),
        ],
        &["s"],
    );
    let out = sweep(
        &search,
        &results,
        FallbackConfig::DEFAULT,
        None,
        &mut SystemClock,
    );
    assert_eq!(named(&search, &out.paths), vec![vec!["s", "a"]]);
    // The self edge is not in the step graph at all, so nothing can walk it.
    let mut scratch = FallbackScratch::new(&search, FallbackConfig::DEFAULT.tie_break);
    scratch.load_step(&search, &results, 1, FallbackWeight::DEFAULT);
    let a = node_id(&search, "a");
    assert!(scratch.adj[a as usize].iter().all(|e| e.node != a));
    assert!(scratch.rev[a as usize].iter().all(|e| e.node != a));
}

// --- Dedup ----------------------------------------------------------------

#[test]
fn the_same_cycle_seen_from_two_stocks_is_emitted_once() {
    let (search, results) = fixture(
        &[("s1", "s2", vec![1.0]), ("s2", "s1", vec![1.0])],
        &["s1", "s2"],
    );
    let out = sweep(
        &search,
        &results,
        FallbackConfig::DEFAULT,
        None,
        &mut SystemClock,
    );
    assert_eq!(named(&search, &out.paths), vec![vec!["s1", "s2"]]);
}

#[test]
fn the_same_cycle_seen_at_two_steps_is_emitted_once() {
    let (search, results) = fixture(
        &[
            ("s", "a", vec![1.0, 1.0, 1.0]),
            ("a", "s", vec![1.0, 1.0, 1.0]),
        ],
        &["s"],
    );
    let out = sweep(
        &search,
        &results,
        FallbackConfig::DEFAULT,
        None,
        &mut SystemClock,
    );
    assert_eq!(out.steps_processed, 3);
    assert_eq!(named(&search, &out.paths), vec![vec!["s", "a"]]);
}

/// Issue #308 semantics for the fallback: two cycles over the same node set
/// but in opposite directions are DIFFERENT loops (opposite polarities, own
/// scores), so canonical-rotation dedup must keep both.
///
/// One Dijkstra tree can only express one of them, so the fixture makes each
/// direction the cheap one at a different step: step 1 charges `a -> b`,
/// step 2 charges `a -> c`.
#[test]
fn opposite_direction_three_cycles_are_both_kept() {
    let (search, results) = fixture(
        &[
            ("a", "b", vec![0.01, 1.0]),
            ("a", "c", vec![1.0, 0.01]),
            ("b", "a", vec![1.0, 1.0]),
            ("b", "c", vec![1.0, 1.0]),
            ("c", "a", vec![1.0, 1.0]),
            ("c", "b", vec![1.0, 1.0]),
        ],
        &["a"],
    );
    let out = sweep(
        &search,
        &results,
        FallbackConfig::DEFAULT,
        None,
        &mut SystemClock,
    );
    let paths = named(&search, &out.paths);
    assert!(
        paths.contains(&vec!["a".into(), "c".into(), "b".into()]),
        "a->c->b->a missing from {paths:?}"
    );
    assert!(
        paths.contains(&vec!["a".into(), "b".into(), "c".into()]),
        "a->b->c->a missing from {paths:?}"
    );
    // The two 2-cycles the step graphs also expose; nothing else.
    assert_eq!(paths.len(), 4, "{paths:?}");
}

// --- Deadline -------------------------------------------------------------

/// Two steps, a different cycle live in each -- so which step the sweep got
/// through is observable in the output.
fn two_step_fixture() -> (IndexedSearch, Results) {
    fixture(
        &[
            ("s", "a", vec![1.0, 0.0]),
            ("a", "s", vec![1.0, 0.0]),
            ("s", "b", vec![0.0, 1.0]),
            ("b", "s", vec![0.0, 1.0]),
        ],
        &["s"],
    )
}

#[test]
fn an_already_expired_deadline_yields_no_paths() {
    let (search, results) = two_step_fixture();
    let mut clock = ScriptedClock::new(1);
    let deadline = clock.deadline();
    let out = sweep(
        &search,
        &results,
        FallbackConfig::DEFAULT,
        Some(deadline),
        &mut clock,
    );
    assert!(out.truncated);
    assert_eq!(out.steps_processed, 0);
    assert!(out.paths.is_empty());
}

#[test]
fn a_deadline_expiring_after_the_first_step_keeps_that_steps_cycles() {
    // Clock reads per step: one at the top of the step, then one before each
    // seed stock's Dijkstra (this fixture has a single stock, on a cycle at
    // both steps). So reads 1 and 2 belong to step 1 and read 3 is step 2's
    // top-of-step check -- expire there and step 1's work survives whole.
    let (search, results) = two_step_fixture();
    let mut clock = ScriptedClock::new(3);
    let deadline = clock.deadline();
    let out = sweep(
        &search,
        &results,
        FallbackConfig::DEFAULT,
        Some(deadline),
        &mut clock,
    );
    assert_eq!(
        clock.reads, 3,
        "the read schedule this test is calibrated to"
    );
    assert!(out.truncated, "partial results still report truncation");
    assert_eq!(out.steps_processed, 1);
    assert_eq!(named(&search, &out.paths), vec![vec!["s", "a"]]);
}

#[test]
fn a_deadline_expiring_between_seed_stocks_completes_no_step() {
    // Two stocks, one disjoint cycle each, so the sweep reads the clock at the
    // top of step 1 (read 1) and then before each stock's search (reads 2 and
    // 3). Expiring at read 3 lands mid-step: the first stock's cycle is
    // already found and is kept, but step 1 never finished, so it does not
    // count as processed -- `steps_processed` is whole steps, not started ones.
    let (search, results) = fixture(
        &[
            ("s1", "a", vec![1.0]),
            ("a", "s1", vec![1.0]),
            ("s2", "b", vec![1.0]),
            ("b", "s2", vec![1.0]),
        ],
        &["s1", "s2"],
    );
    let mut clock = ScriptedClock::new(3);
    let deadline = clock.deadline();
    let out = sweep(
        &search,
        &results,
        FallbackConfig::DEFAULT,
        Some(deadline),
        &mut clock,
    );
    assert_eq!(
        clock.reads, 3,
        "the read schedule this test is calibrated to"
    );
    assert!(out.truncated);
    assert_eq!(out.steps_processed, 0);
    assert_eq!(named(&search, &out.paths), vec![vec!["s1", "a"]]);
}

#[test]
fn a_deadline_expiring_inside_a_search_keeps_what_it_can_already_close() {
    // The third place the sweep reads the clock: inside a single search, so a
    // seed whose component is the whole graph cannot overrun the budget on its
    // own. Shrinking the pop interval to 1 makes a two-node cycle exercise it.
    //
    // Reads: step 1's top (1), the pre-search check (2), then one per pop --
    // the seed (3) and `a` (4). Expiring at read 4 cuts the search after `a`
    // was reached but before it settled, which still closes `s -> a -> s`:
    // the parent tree is valid at every instant, so a truncated search returns
    // real loops rather than nothing.
    //
    // Pinned at the single-search closure family, because that is what makes
    // the schedule three-place; the two-search family's own schedule is the
    // next test.
    let _guard = DeadlinePopIntervalGuard::new(1);
    let (search, results) = two_step_fixture();
    let mut clock = ScriptedClock::new(4);
    let deadline = clock.deadline();
    let out = sweep(
        &search,
        &results,
        in_edge_closures(FallbackWeight::DEFAULT),
        Some(deadline),
        &mut clock,
    );
    assert_eq!(
        clock.reads, 4,
        "the read schedule this test is calibrated to"
    );
    assert!(out.truncated);
    assert_eq!(out.steps_processed, 0, "step 1 never finished");
    assert_eq!(named(&search, &out.paths), vec![vec!["s", "a"]]);
}

/// The every-edge family runs a SECOND search per seed, and it must run even
/// when the first was cut short: the closures read a forward tree and a
/// reverse tree together, and a reverse tree left over from the previous seed
/// would splice two unrelated trees into a walk that is no cycle at all.
///
/// Reads here: step 1's top (1), the pre-search check (2), the forward
/// search's two pops (3, 4 -- expiring at 4), then the reverse search's first
/// pop (5), which sees the expiry and returns immediately. The reverse tree it
/// leaves behind holds only the seed, which is enough to close `s -> a -> s`
/// through the in-edge case.
#[test]
fn a_deadline_expiring_inside_a_search_still_runs_the_reverse_search() {
    let _guard = DeadlinePopIntervalGuard::new(1);
    let (search, results) = two_step_fixture();
    let mut clock = ScriptedClock::new(4);
    let deadline = clock.deadline();
    let out = sweep(
        &search,
        &results,
        FallbackConfig {
            closures: FallbackClosures::EveryEdge,
            ..FallbackConfig::DEFAULT
        },
        Some(deadline),
        &mut clock,
    );
    assert_eq!(
        clock.reads, 5,
        "the read schedule this test is calibrated to"
    );
    assert!(out.truncated);
    assert_eq!(out.steps_processed, 0, "step 1 never finished");
    assert_eq!(named(&search, &out.paths), vec![vec!["s", "a"]]);
}

#[test]
fn an_unbudgeted_sweep_never_reads_the_clock() {
    let (search, results) = two_step_fixture();
    // Scripted to expire on its very first read: if the sweep ever consulted
    // it, the run would truncate immediately.
    let mut clock = ScriptedClock::new(1);
    let out = sweep(&search, &results, FallbackConfig::DEFAULT, None, &mut clock);
    assert_eq!(clock.reads, 0);
    assert!(!out.truncated);
    assert_eq!(out.steps_processed, 2);
    assert_eq!(
        named(&search, &out.paths),
        vec![vec!["s", "a"], vec!["s", "b"]]
    );
}

// --- Candidate budget -------------------------------------------------------

/// A tiny cap trips mid-seed: [`CycleDedup::insert_if_new`] refuses every
/// candidate once `paths` reaches the cap, so of `three_way_fixture`'s three
/// cycles through `s` only the first found (the cheapest, under
/// `ClampedLogAbs`'s minimum-first closing order) survives. The sweep only
/// notices AFTER the seed's closings loop has tried all three -- the check is
/// once per seed, not once per candidate -- so the reported `truncated` and
/// `paths` are the observable trip rather than an early return mid closing.
#[test]
fn a_tiny_candidate_budget_trips_and_keeps_only_what_fit() {
    let _guard = MaxFallbackPathsGuard::new(1);
    let (search, results) = three_way_fixture();
    let out = sweep(
        &search,
        &results,
        in_edge_closures(FallbackWeight::ClampedLogAbs),
        None,
        &mut SystemClock,
    );
    assert!(out.truncated);
    assert_eq!(out.steps_processed, 0, "the only step never finished");
    assert_eq!(named(&search, &out.paths), vec![vec!["s", "b", "c"]]);
}

/// The cap is the smaller of the count bound and the materialization byte
/// budget: at World3's 401 saved steps the count binds (20,000 candidates is
/// ~128 MB), while at 100,000 saved steps a fixed count would let the
/// materialized series climb to ~32 GB, so the byte budget takes over and
/// the cap falls with the run length. Never below one candidate.
#[test]
fn the_candidate_cap_shrinks_with_the_saved_step_count() {
    let per = |steps: usize| steps * BYTES_PER_MATERIALIZED_STEP;
    assert_eq!(max_fallback_paths(401), MAX_FALLBACK_PATHS);
    assert_eq!(
        max_fallback_paths(100_000),
        MAX_FALLBACK_MATERIALIZATION_BYTES / per(100_000)
    );
    assert!(max_fallback_paths(100_000) < MAX_FALLBACK_PATHS);
    // The count and the byte budget swap roles exactly where the series of
    // MAX_FALLBACK_PATHS candidates fill the budget.
    let crossover =
        MAX_FALLBACK_MATERIALIZATION_BYTES / (MAX_FALLBACK_PATHS * BYTES_PER_MATERIALIZED_STEP);
    assert_eq!(max_fallback_paths(crossover), MAX_FALLBACK_PATHS);
    assert!(max_fallback_paths(crossover + 1) < MAX_FALLBACK_PATHS);
    assert_eq!(max_fallback_paths(usize::MAX / 32), 1, "never below one");
    assert_eq!(max_fallback_paths(0), max_fallback_paths(1));
}

/// The not-tripped control for the test above: a cap comfortably above the
/// fixture's candidate count changes nothing about the sweep's output.
#[test]
fn a_candidate_budget_with_headroom_leaves_the_sweep_untouched() {
    let _guard = MaxFallbackPathsGuard::new(4);
    let (search, results) = three_way_fixture();
    let out = sweep(
        &search,
        &results,
        in_edge_closures(FallbackWeight::ClampedLogAbs),
        None,
        &mut SystemClock,
    );
    assert!(!out.truncated);
    assert_eq!(out.steps_processed, 1);
    assert_eq!(
        named(&search, &out.paths),
        vec![vec!["s", "b", "c"], vec!["s", "d", "e"], vec!["s", "a"]],
    );
}

// --- Determinism ----------------------------------------------------------

#[test]
fn sweep_output_is_content_pure() {
    let (search, results) = three_way_fixture();
    for arm in WEIGHT_ARMS {
        let config = FallbackConfig::with_weight(arm);
        let first = sweep(&search, &results, config, None, &mut SystemClock);
        let second = sweep(&search, &results, config, None, &mut SystemClock);
        let cheap_first = sweep(
            &search,
            &results,
            in_edge_closures(arm),
            None,
            &mut SystemClock,
        );
        let cheap_second = sweep(
            &search,
            &results,
            in_edge_closures(arm),
            None,
            &mut SystemClock,
        );
        assert_eq!(
            cheap_first.paths, cheap_second.paths,
            "{arm:?} in-edge closures are order-unstable"
        );
        assert_eq!(first.paths, second.paths, "{arm:?} is order-unstable");
        assert_eq!(first.steps_processed, second.steps_processed);
        assert_eq!(first.truncated, second.truncated);
    }
}

#[test]
fn an_empty_graph_sweeps_to_nothing() {
    let (search, results) = fixture(&[], &["s"]);
    let out = sweep(
        &search,
        &results,
        FallbackConfig::DEFAULT,
        None,
        &mut SystemClock,
    );
    assert!(out.paths.is_empty());
    assert!(!out.truncated);
    assert_eq!(out.steps_processed, 0);
}
