// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Unit tests for `ltm_finding_fallback.rs`, split out of the module body to
//! keep the production file under the per-file line cap (mounted via `#[path]`).

use std::collections::HashMap;
use std::time::Duration;

use super::super::LinkOffset;
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
    let &(weight, from) = closings.first()?;
    let mut path = Vec::new();
    scratch.reconstruct_path(seed, from, &mut path);
    Some((weight, path))
}

// --- Weight formulations --------------------------------------------------

#[test]
fn default_weight_is_clamped_log_abs() {
    // The design's starting formulation: the one arm that keeps Dijkstra's
    // non-negativity precondition without needing a per-target normalization.
    assert_eq!(FallbackWeight::DEFAULT, FallbackWeight::ClampedLogAbs);
}

#[test]
fn clamped_log_abs_charges_sub_unit_links_and_frees_super_unit_ones() {
    let w = |abs| edge_weight(FallbackWeight::ClampedLogAbs, abs, f64::NAN);
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
    assert_eq!(edge_weight(FallbackWeight::ClampedLogAbs, 2.0, 1.0), 0.0);
}

#[test]
fn hop_count_charges_one_per_hop_regardless_of_score() {
    let w = |abs, sum| edge_weight(FallbackWeight::HopCount, abs, sum);
    assert_eq!(w(1e-9, 1e-9), 1.0);
    assert_eq!(w(1.0, 2.0), 1.0);
    assert_eq!(w(1e9, 1e9), 1.0);
    assert_eq!(w(f64::INFINITY, f64::INFINITY), 1.0);
}

#[test]
fn relative_link_score_normalizes_against_the_targets_in_edges() {
    let w = |abs, sum| edge_weight(FallbackWeight::RelativeLinkScore, abs, sum);
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
    let w = |abs, sum| edge_weight(FallbackWeight::RelativeLinkScore, abs, sum);
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
    let mut scratch = FallbackScratch::new(&search);
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
    let arms = [
        FallbackWeight::ClampedLogAbs,
        FallbackWeight::RelativeLinkScore,
        FallbackWeight::HopCount,
    ];
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
        let mut scratch = FallbackScratch::new(&search);

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
    let out = sweep(&search, &results, FallbackWeight::ClampedLogAbs, None);
    // Every cycle through the stock is proposed; the minimum comes first.
    assert_eq!(
        named(&search, &out.paths),
        vec![vec!["s", "b", "c"], vec!["s", "d", "e"], vec!["s", "a"],],
    );
}

#[test]
fn hop_count_prefers_the_shortest_path() {
    let (search, results) = three_way_fixture();
    let out = sweep(&search, &results, FallbackWeight::HopCount, None);
    assert_eq!(
        named(&search, &out.paths),
        vec![vec!["s", "a"], vec!["s", "b", "c"], vec!["s", "d", "e"],],
    );
}

#[test]
fn relative_link_score_prefers_the_largest_share_of_the_stocks_in_edges() {
    let (search, results) = three_way_fixture();
    let out = sweep(&search, &results, FallbackWeight::RelativeLinkScore, None);
    assert_eq!(
        named(&search, &out.paths),
        vec![vec!["s", "d", "e"], vec!["s", "b", "c"], vec!["s", "a"],],
    );

    // Hand-computed normalization: `s` has in-edges a->s (0.5), c->s (4.0)
    // and e->s (10.0), so its determinant mass is 14.5. Every other node has
    // a single determinant, so all non-closing hops are free and each cycle's
    // weight is just -ln(share) of its closing edge.
    let mut scratch = FallbackScratch::new(&search);
    scratch.load_step(&search, &results, 1, FallbackWeight::RelativeLinkScore);
    let s = node_id(&search, "s");
    assert_eq!(scratch.in_sum[s as usize], 14.5);
    let completed = scratch.dijkstra_from(s, None, &mut SystemClock);
    assert!(completed);
    let mut closings = Vec::new();
    scratch.collect_closings(s, &mut closings);
    let weights: Vec<f64> = closings.iter().map(|&(w, _)| w).collect();
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
    let out = sweep(&search, &results, FallbackWeight::DEFAULT, None);
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

// --- Self edges -----------------------------------------------------------

#[test]
fn a_stock_with_only_a_self_edge_yields_no_cycle() {
    // A one-variable "loop" is not a feedback loop in the SD sense, and the
    // enumerator does not emit one either -- the two generators must agree on
    // what a loop is.
    let (search, results) = fixture(&[("s", "s", vec![2.0])], &["s"]);
    let out = sweep(&search, &results, FallbackWeight::DEFAULT, None);
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
    let out = sweep(&search, &results, FallbackWeight::DEFAULT, None);
    assert_eq!(named(&search, &out.paths), vec![vec!["s", "a"]]);
    // The self edge is not in the step graph at all, so nothing can walk it.
    let mut scratch = FallbackScratch::new(&search);
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
    let out = sweep(&search, &results, FallbackWeight::DEFAULT, None);
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
    let out = sweep(&search, &results, FallbackWeight::DEFAULT, None);
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
    let out = sweep(&search, &results, FallbackWeight::DEFAULT, None);
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

/// A clock whose expiry is scripted by read index rather than by real time, so
/// mid-sweep truncation is deterministic instead of racing a `Duration`.
struct ScriptedClock {
    base: Instant,
    /// Reads at index >= this one return a time far past any deadline below.
    expire_at_read: usize,
    reads: usize,
}

impl ScriptedClock {
    fn new(expire_at_read: usize) -> Self {
        ScriptedClock {
            base: Instant::now(),
            expire_at_read,
            reads: 0,
        }
    }

    /// A deadline this clock is before until `expire_at_read` reads have gone by.
    fn deadline(&self) -> Instant {
        self.base + Duration::from_secs(1)
    }
}

impl Clock for ScriptedClock {
    fn now(&mut self) -> Instant {
        let index = self.reads;
        self.reads += 1;
        if index + 1 >= self.expire_at_read {
            self.base + Duration::from_secs(3600)
        } else {
            self.base
        }
    }
}

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
    let out = sweep_with_clock(
        &search,
        &results,
        FallbackWeight::DEFAULT,
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
    let out = sweep_with_clock(
        &search,
        &results,
        FallbackWeight::DEFAULT,
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
    let out = sweep_with_clock(
        &search,
        &results,
        FallbackWeight::DEFAULT,
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
    let _guard = DeadlinePopIntervalGuard::new(1);
    let (search, results) = two_step_fixture();
    let mut clock = ScriptedClock::new(4);
    let deadline = clock.deadline();
    let out = sweep_with_clock(
        &search,
        &results,
        FallbackWeight::DEFAULT,
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

#[test]
fn an_unbudgeted_sweep_never_reads_the_clock() {
    let (search, results) = two_step_fixture();
    // Scripted to expire on its very first read: if the sweep ever consulted
    // it, the run would truncate immediately.
    let mut clock = ScriptedClock::new(1);
    let out = sweep_with_clock(&search, &results, FallbackWeight::DEFAULT, None, &mut clock);
    assert_eq!(clock.reads, 0);
    assert!(!out.truncated);
    assert_eq!(out.steps_processed, 2);
    assert_eq!(
        named(&search, &out.paths),
        vec![vec!["s", "a"], vec!["s", "b"]]
    );
}

// --- Determinism ----------------------------------------------------------

#[test]
fn sweep_output_is_content_pure() {
    let (search, results) = three_way_fixture();
    for arm in [
        FallbackWeight::ClampedLogAbs,
        FallbackWeight::RelativeLinkScore,
        FallbackWeight::HopCount,
    ] {
        let first = sweep(&search, &results, arm, None);
        let second = sweep(&search, &results, arm, None);
        assert_eq!(first.paths, second.paths, "{arm:?} is order-unstable");
        assert_eq!(first.steps_processed, second.steps_processed);
        assert_eq!(first.truncated, second.truncated);
    }
}

#[test]
fn an_empty_graph_sweeps_to_nothing() {
    let (search, results) = fixture(&[], &["s"]);
    let out = sweep(&search, &results, FallbackWeight::DEFAULT, None);
    assert!(out.paths.is_empty());
    assert!(!out.truncated);
    assert_eq!(out.steps_processed, 0);
}
