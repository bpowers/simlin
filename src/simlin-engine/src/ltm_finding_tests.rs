// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Unit tests for `ltm_finding.rs`, split out of the module body to keep the
//! production file under the per-file line cap (mounted via `#[path]`).

use super::*;
use crate::common::canonicalize;

/// Helper to build edges from tuples
fn edges(tuples: &[(&str, &str, f64)]) -> Vec<(Ident<Canonical>, Ident<Canonical>, f64)> {
    tuples
        .iter()
        .map(|(from, to, score)| (Ident::new(from), Ident::new(to), *score))
        .collect()
}

/// Helper to build stock list from names
fn stock_list(names: &[&str]) -> Vec<Ident<Canonical>> {
    names.iter().map(|n| Ident::new(n)).collect()
}

/// Helper to extract sorted node set from a path for comparison
fn sorted_node_set(path: &[Ident<Canonical>]) -> Vec<String> {
    let mut set: Vec<String> = path.iter().map(|id| id.as_str().to_string()).collect();
    set.sort();
    set
}

// --- collapse_synthetic_links ---

fn clink(from: &str, to: &str, polarity: LinkPolarity, score: Option<Vec<f64>>) -> CollapsibleLink {
    CollapsibleLink {
        from: Ident::new(from),
        to: Ident::new(to),
        polarity,
        score,
    }
}

/// Look up a collapsed edge by (from, to) in the result.
fn find_edge<'a>(
    links: &'a [CollapsibleLink],
    from: &str,
    to: &str,
) -> Option<&'a CollapsibleLink> {
    links
        .iter()
        .find(|l| l.from.as_str() == from && l.to.as_str() == to)
}

#[test]
fn collapse_passes_through_a_graph_with_no_synthetic_nodes() {
    // A purely real graph is returned unchanged (modulo nothing).
    let input = vec![
        clink("a", "b", LinkPolarity::Positive, Some(vec![1.0, 2.0])),
        clink("b", "c", LinkPolarity::Negative, Some(vec![3.0, 4.0])),
    ];
    let out = collapse_synthetic_links(input);
    assert_eq!(out.len(), 2);
    assert!(find_edge(&out, "a", "b").is_some());
    assert!(find_edge(&out, "b", "c").is_some());
}

#[test]
fn collapse_single_chain_through_a_macro_node() {
    // Mirrors the SMTH1 edge structure from model_causal_edges:
    //   level -> $⁚smoothed_level⁚0⁚smth1 -> smoothed_level
    // plus a dangling synthetic arg helper feeding the module that has no
    // real predecessor. The chain collapses to one composite edge
    // `level -> smoothed_level` (product polarity, product score); the
    // arg-helper chain is dropped (no real source).
    let smth = "$\u{205A}smoothed_level\u{205A}0\u{205A}smth1";
    let arg = "$\u{205A}smoothed_level\u{205A}0\u{205A}arg1";
    let input = vec![
        clink("level", smth, LinkPolarity::Positive, Some(vec![2.0, -3.0])),
        clink(
            smth,
            "smoothed_level",
            LinkPolarity::Negative,
            Some(vec![5.0, 7.0]),
        ),
        clink(arg, smth, LinkPolarity::Positive, Some(vec![9.0, 9.0])),
    ];
    let out = collapse_synthetic_links(input);
    // No synthetic node survives.
    assert!(
        out.iter()
            .all(|l| !l.from.as_str().starts_with('$') && !l.to.as_str().starts_with('$')),
        "no synthetic node should remain: {:?}",
        out.iter()
            .map(|l| (l.from.as_str(), l.to.as_str()))
            .collect::<Vec<_>>()
    );
    // The composite `level -> smoothed_level` carries product polarity and
    // per-step product score.
    let edge =
        find_edge(&out, "level", "smoothed_level").expect("level -> smoothed_level composite edge");
    assert_eq!(edge.polarity, LinkPolarity::Negative); // + composed with -
    assert_eq!(edge.score.as_deref(), Some(&[10.0, -21.0][..]));
    // The arg-helper chain produced no edge (it has no real source).
    assert_eq!(out.len(), 1);
}

#[test]
fn collapse_picks_max_magnitude_path_score() {
    // Two disjoint synthetic paths from a -> z. The composite link score is
    // the per-timestep larger-magnitude path score (ref 6.3); the reported
    // polarity follows the dominant path.
    let s1 = "$\u{205A}m\u{205A}0\u{205A}f"; // path 1 internal
    let s2 = "$\u{205A}m\u{205A}1\u{205A}g"; // path 2 internal
    let input = vec![
        // path 1: a -> s1 -> z, scores 1*1 and 1*1 = [1, 1], Positive
        clink("a", s1, LinkPolarity::Positive, Some(vec![1.0, 1.0])),
        clink(s1, "z", LinkPolarity::Positive, Some(vec![1.0, 1.0])),
        // path 2: a -> s2 -> z, scores 10*1 and 0.5*0.5 = [10, 0.25], Negative
        clink("a", s2, LinkPolarity::Negative, Some(vec![10.0, 0.5])),
        clink(s2, "z", LinkPolarity::Positive, Some(vec![1.0, 0.5])),
    ];
    let out = collapse_synthetic_links(input);
    let edge = find_edge(&out, "a", "z").expect("a -> z composite");
    // step 0: |10| > |1| -> path 2 (10, Negative); step 1: |1| > |0.25| ->
    // path 1 (1). Max-abs keeps the per-step winner's sign.
    assert_eq!(edge.score.as_deref(), Some(&[10.0, 1.0][..]));
    // Aggregate magnitude: path2 sum |10|+|0.25| = 10.25 > path1 sum 2.0,
    // so the dominant-path polarity is Negative.
    assert_eq!(edge.polarity, LinkPolarity::Negative);
}

#[test]
fn collapse_drops_a_fully_internal_cycle() {
    // A synthetic-only cycle (s1 -> s2 -> s1) with no real entry/exit must
    // not loop forever and must produce no user-visible edge.
    let s1 = "$\u{205A}m\u{205A}0\u{205A}f";
    let s2 = "$\u{205A}m\u{205A}1\u{205A}g";
    let input = vec![
        clink(s1, s2, LinkPolarity::Positive, Some(vec![1.0])),
        clink(s2, s1, LinkPolarity::Positive, Some(vec![1.0])),
    ];
    let out = collapse_synthetic_links(input);
    assert!(out.is_empty(), "fully-internal cycle yields no edges");
}

#[test]
fn collapse_structural_only_path_has_no_scores() {
    // No score series (structural-only caller): the composite still
    // collapses, polarity composes, and the score stays None.
    let smth = "$\u{205A}v\u{205A}0\u{205A}smth1";
    let input = vec![
        clink("x", smth, LinkPolarity::Negative, None),
        clink(smth, "y", LinkPolarity::Negative, None),
    ];
    let out = collapse_synthetic_links(input);
    let edge = find_edge(&out, "x", "y").expect("x -> y composite");
    assert_eq!(edge.polarity, LinkPolarity::Positive); // - composed with -
    assert!(edge.score.is_none());
}

#[test]
fn collapse_folds_two_disagreeing_structural_paths_to_unknown() {
    // Two scoreless (structural-only) paths reach the same real endpoint
    // with disagreeing polarity, and the FIRST is genuinely Unknown:
    //   a --Unknown--> c                          (direct)
    //   a --+--> $synth --+--> c                  (composes to Positive)
    // The merged edge must be Unknown (two disagreeing structural paths,
    // per pick_stronger_polarity's both-None arm). Regression guard: when
    // (Unknown, None) doubled as the uninitialized map sentinel, the first
    // path was silently overwritten and the edge wrongly reported Positive.
    let smth = "$\u{205A}v\u{205A}0\u{205A}smth1";
    let input = vec![
        clink("a", "c", LinkPolarity::Unknown, None),
        clink("a", smth, LinkPolarity::Positive, None),
        clink(smth, "c", LinkPolarity::Positive, None),
    ];
    let out = collapse_synthetic_links(input);
    let edge = find_edge(&out, "a", "c").expect("a -> c composite");
    assert_eq!(edge.polarity, LinkPolarity::Unknown);
    assert!(edge.score.is_none());
}

// --- Test 1: SearchGraph construction ---

#[test]
fn test_search_graph_construction() {
    let graph = SearchGraph::from_edges(
        edges(&[
            ("a", "b", 10.0),
            ("a", "d", 100.0),
            ("b", "c", 10.0),
            ("c", "a", 10.0),
            ("d", "c", 0.1),
            ("d", "b", 100.0),
        ]),
        stock_list(&["a", "b", "c", "d"]),
    );

    // Verify adjacency list exists for all source nodes
    assert!(graph.adj.contains_key(&*canonicalize("a")));
    assert!(graph.adj.contains_key(&*canonicalize("b")));
    assert!(graph.adj.contains_key(&*canonicalize("c")));
    assert!(graph.adj.contains_key(&*canonicalize("d")));

    // Verify edges are sorted by |score| descending
    let a_edges = &graph.adj[&*canonicalize("a")];
    assert_eq!(a_edges.len(), 2);
    assert_eq!(a_edges[0].to.as_str(), "d"); // score 100
    assert_eq!(a_edges[1].to.as_str(), "b"); // score 10

    let d_edges = &graph.adj[&*canonicalize("d")];
    assert_eq!(d_edges.len(), 2);
    assert_eq!(d_edges[0].to.as_str(), "b"); // score 100
    assert_eq!(d_edges[1].to.as_str(), "c"); // score 0.1

    // Verify stocks
    assert_eq!(graph.stocks.len(), 4);
}

// --- Test 2: Trivial loop ---

#[test]
fn test_trivial_loop() {
    // Single stock with a flow forming one loop: stock -> flow -> stock
    let graph = SearchGraph::from_edges(
        edges(&[("stock", "flow", 1.0), ("flow", "stock", 1.0)]),
        stock_list(&["stock"]),
    );

    let loops = graph.find_strongest_loops();
    assert_eq!(loops.len(), 1, "Should find exactly one loop");

    let loop_nodes = sorted_node_set(&loops[0]);
    assert_eq!(loop_nodes, vec!["flow", "stock"]);
}

// --- Test 3: Figure 7 from the paper ---

#[test]
fn test_figure_7_paper() {
    // Edges from the paper's Figure 7:
    // a->b:10, a->d:100, b->c:10, c->a:10, d->c:0.1, d->b:100
    // All nodes are stocks for this test.
    let graph = SearchGraph::from_edges(
        edges(&[
            ("a", "b", 10.0),
            ("a", "d", 100.0),
            ("b", "c", 10.0),
            ("c", "a", 10.0),
            ("d", "c", 0.1),
            ("d", "b", 100.0),
        ]),
        stock_list(&["a", "b", "c", "d"]),
    );

    let loops = graph.find_strongest_loops();

    // The paper's Figure 7 demonstrates the original heuristic's failure
    // mode: with `best_score` pruning, the strong path a->d sets scores
    // that prune the weaker a->b entry, missing the a->b->c->a loop when
    // searching from stock a (the paper recovers it via per-stock reset).
    // With expansion-cap-bounded search, all three loops are found
    // exhaustively -- the small-graph case is strictly more complete.
    assert_eq!(
        loops.len(),
        3,
        "Figure 7: should find all 3 loops, found {}",
        loops.len()
    );

    let mut loop_sets: Vec<Vec<String>> = loops.iter().map(|l| sorted_node_set(l)).collect();
    loop_sets.sort();
    assert_eq!(
        loop_sets,
        vec![
            vec!["a", "b", "c"],
            vec!["a", "b", "c", "d"],
            vec!["a", "c", "d"],
        ],
    );
}

// --- Test 4: per-stock search isolation ---

#[test]
fn test_per_stock_search_isolation() {
    // Graph:
    //   a -> x (score 1000)
    //   x -> a (score 1000)  -- strong loop through a
    //   b -> x (score 1)     -- weak path from b
    //   x -> b (score 1)     -- weak path back
    //
    // Per-stock state isolation (the paper's per-stock `best_score`
    // reset, here per-stock expansion-count reset): one stock's search
    // must not limit loops reachable from another stock.
    //
    // TARGET=a: finds [a, x] (strong loop)
    // TARGET=b: fresh expansion counts, finds [b, x] (weak loop)
    let graph = SearchGraph::from_edges(
        edges(&[
            ("a", "x", 1000.0),
            ("x", "a", 1000.0),
            ("x", "b", 1.0),
            ("b", "x", 1.0),
        ]),
        stock_list(&["a", "b"]),
    );

    let loops = graph.find_strongest_loops();

    assert_eq!(
        loops.len(),
        2,
        "Per-stock isolation should find both loops, found {}",
        loops.len()
    );

    let mut loop_sets: Vec<Vec<String>> = loops.iter().map(|l| sorted_node_set(l)).collect();
    loop_sets.sort();
    assert_eq!(loop_sets, vec![vec!["a", "x"], vec!["b", "x"]]);
}

// --- Test 5: Loop deduplication ---

#[test]
fn test_loop_deduplication() {
    // Stock a and stock b both participate in the same loop (a -> b -> a);
    // the canonical-rotation dedup must report it only once even though
    // both per-stock searches traverse it.
    let graph = SearchGraph::from_edges(
        edges(&[("a", "b", 1.0), ("b", "a", 1.0)]),
        stock_list(&["a", "b"]),
    );

    let loops = graph.find_strongest_loops();

    // Even though both stocks can reach the loop, deduplication should ensure
    // it appears only once
    assert_eq!(loops.len(), 1, "Same loop should appear only once");

    let loop_nodes = sorted_node_set(&loops[0]);
    assert_eq!(loop_nodes, vec!["a", "b"]);
}

/// Issue #308 regression test for `add_loop_if_unique`:
/// the discovery DFS must keep both directions of a directed
/// 3-cycle as distinct loops when they share a node set.
///
/// We exercise the helper directly so the dedup-key property is
/// pinned independently of which paths the DFS happens to surface.
/// Calling `add_loop_if_unique` with the two paths is a precise
/// check that the dedup key distinguishes them.
#[test]
fn add_loop_if_unique_keeps_distinct_directed_three_cycles() {
    let mut found_loops: Vec<Vec<Ident<Canonical>>> = Vec::new();
    let mut seen: HashSet<Vec<String>> = HashSet::new();

    let forward: Vec<Ident<Canonical>> = vec![Ident::new("a"), Ident::new("b"), Ident::new("c")];
    let reverse: Vec<Ident<Canonical>> = vec![Ident::new("a"), Ident::new("c"), Ident::new("b")];

    SearchGraph::add_loop_if_unique(&forward, &mut found_loops, &mut seen);
    SearchGraph::add_loop_if_unique(&reverse, &mut found_loops, &mut seen);

    assert_eq!(
        found_loops.len(),
        2,
        "opposite-direction 3-cycles must be retained as distinct loops"
    );
    assert_eq!(found_loops[0], forward);
    assert_eq!(found_loops[1], reverse);

    // Calling again with a rotation of one of the existing cycles
    // must still dedup (rotations of the same directed cycle
    // canonicalize to the same key).
    let forward_rotation: Vec<Ident<Canonical>> =
        vec![Ident::new("b"), Ident::new("c"), Ident::new("a")];
    SearchGraph::add_loop_if_unique(&forward_rotation, &mut found_loops, &mut seen);
    assert_eq!(
        found_loops.len(),
        2,
        "a rotation of an already-seen directed cycle must be deduped"
    );
}

// --- Test 6: Empty graph ---

#[test]
fn test_empty_graph() {
    let graph = SearchGraph::from_edges(vec![], stock_list(&[]));
    let loops = graph.find_strongest_loops();
    assert!(loops.is_empty(), "Empty graph should have no loops");
}

#[test]
fn test_no_edges() {
    // Graph with stocks but no edges
    let graph = SearchGraph::from_edges(vec![], stock_list(&["a", "b"]));
    let loops = graph.find_strongest_loops();
    assert!(loops.is_empty(), "Graph with no edges should have no loops");
}

// --- Test 7: Zero-score edges ---

#[test]
fn test_zero_score_edges() {
    // A link with score 0 means the causal connection is inactive at this
    // timestep: any loop through it has loop score exactly 0 here, so it
    // is not a "loop that matters" at this step. Zero-score edges are
    // therefore excluded from the per-step search graph (GH #647) -- on
    // real models they are the overwhelming majority of edges, and
    // traversing them is what made discovery wander the whole graph.
    let graph = SearchGraph::from_edges(
        edges(&[
            ("a", "b", 0.0), // zero-score link: inactive at this step
            ("b", "a", 10.0),
        ]),
        stock_list(&["a"]),
    );

    let loops = graph.find_strongest_loops();

    assert!(
        loops.is_empty(),
        "a loop with a zero-score link is inactive at this step and not discovered here"
    );
}

/// The flip side of `test_zero_score_edges`: a loop inactive at one step
/// (zero-score link) is discovered at the step where all its links carry
/// nonzero scores. Discovery runs at every sampled timestep, so per-step
/// exclusion of inactive edges loses no loop that is ever simultaneously
/// active at a sampled step. (Loops whose links are only ever active at
/// different steps are missed -- GH #699.)
#[test]
fn test_inactive_loop_found_at_active_step() {
    let mut offsets = HashMap::new();
    offsets.insert(Ident::new("$⁚ltm⁚link_score⁚a→b"), 0usize);
    offsets.insert(Ident::new("$⁚ltm⁚link_score⁚b→a"), 1usize);
    let data = vec![
        f64::NAN,
        f64::NAN, // step 0 (skipped)
        0.0,
        10.0, // step 1: a->b inactive; loop not discoverable here
        0.5,
        10.0, // step 2: both links active; loop discovered
    ];
    let results = Results {
        offsets,
        data: data.into_boxed_slice(),
        step_size: 2,
        step_count: 3,
        specs: crate::results::Specs {
            start: 0.0,
            stop: 2.0,
            dt: 1.0,
            save_step: 1.0,
            method: crate::results::Method::Euler,
            n_chunks: 3,
        },
        is_vensim: false,
    };
    let link_offsets = parse_link_offsets(&results, &[], &[]);
    let stocks = stock_list(&["a"]);
    let paths = indexed_all_paths(&results, &link_offsets, &stocks);
    assert_eq!(
        paths_as_strings(&paths),
        vec![vec!["a".to_string(), "b".to_string()]],
        "the loop must be discovered at the step where it is active"
    );
}

// --- Test 8: NaN handling ---

#[test]
fn test_nan_handling() {
    // NaN scores are treated as 0 -- the link is inactive at this step,
    // so the loop through it is not discovered here (see
    // `test_zero_score_edges`).
    let graph = SearchGraph::from_edges(
        edges(&[("a", "b", f64::NAN), ("b", "a", 10.0)]),
        stock_list(&["a"]),
    );

    let loops = graph.find_strongest_loops();

    assert!(
        loops.is_empty(),
        "NaN is treated as 0: the loop through it is inactive at this step"
    );
}

// --- GH #647: SCC restriction and bounded re-expansion ---

/// A large acyclic appendage hanging off a small cyclic core must not
/// affect which loops are found: the DFS is restricted to each stock's
/// SCC, and the appendage (reachable from the core, but with no path
/// back) is outside every SCC.
#[test]
fn test_scc_restriction_preserves_core_loops() {
    // Cyclic core: a -> b -> c -> a, plus the shortcut b -> a.
    let mut typed_edges: Vec<(Ident<Canonical>, Ident<Canonical>, f64)> = edges(&[
        ("a", "b", 2.0),
        ("b", "c", 3.0),
        ("c", "a", 4.0),
        ("b", "a", 5.0),
    ]);
    // Acyclic appendage reachable from the core: c -> t0 -> t1 -> ... -> t9.
    typed_edges.push((Ident::new("c"), Ident::new("t0"), 1.0));
    for i in 0..9 {
        typed_edges.push((
            Ident::new(&format!("t{i}")),
            Ident::new(&format!("t{}", i + 1)),
            1.0,
        ));
    }

    let graph = SearchGraph::from_edges(typed_edges, stock_list(&["a"]));
    let loops = graph.find_strongest_loops();

    let mut loop_sets: Vec<Vec<String>> = loops.iter().map(|l| sorted_node_set(l)).collect();
    loop_sets.sort();
    assert_eq!(
        loop_sets,
        vec![vec!["a", "b"], vec!["a", "b", "c"]],
        "both core loops are found; the acyclic tail changes nothing"
    );
}

/// A chain of "diamonds" with tied scores has exponentially many equal-
/// score paths; without bounded re-expansion the DFS re-walks each
/// diamond's subtree once per arriving path (2^k for k diamonds). The
/// expansion cap bounds the work while the loop through the chain is
/// still found.
#[test]
fn test_tied_score_diamond_chain_completes() {
    // stock -> d0 -> {x0, y0} -> d1 -> {x1, y1} -> d2 -> ... -> d24 -> stock
    // All scores 1.0 (the exact-tie case that defeats strict-less-than
    // pruning). 24 diamonds = 2^24 = ~16.7M equal-score paths; without the
    // cap this test would not complete in any reasonable time.
    let n_diamonds = 24;
    let mut names: Vec<String> = vec!["stock".to_string()];
    let mut edge_list: Vec<(String, String, f64)> = Vec::new();
    edge_list.push(("stock".to_string(), "d0".to_string(), 1.0));
    for i in 0..n_diamonds {
        let d = format!("d{i}");
        let x = format!("x{i}");
        let y = format!("y{i}");
        let next = if i + 1 == n_diamonds {
            "stock".to_string()
        } else {
            format!("d{}", i + 1)
        };
        edge_list.push((d.clone(), x.clone(), 1.0));
        edge_list.push((d.clone(), y.clone(), 1.0));
        edge_list.push((x.clone(), next.clone(), 1.0));
        edge_list.push((y.clone(), next.clone(), 1.0));
        names.push(d);
        names.push(x);
        names.push(y);
    }

    let typed_edges: Vec<(Ident<Canonical>, Ident<Canonical>, f64)> = edge_list
        .iter()
        .map(|(f, t, s)| (Ident::new(f), Ident::new(t), *s))
        .collect();
    let graph = SearchGraph::from_edges(typed_edges, stock_list(&["stock"]));

    let loops = graph.find_strongest_loops();
    // At least one loop through the diamond chain is found (each found
    // loop picks one arm per diamond), and the search completes -- which
    // is the property under test.
    assert!(
        !loops.is_empty(),
        "the loop through the diamond chain must be found"
    );
    for l in &loops {
        // Each loop visits the stock, every diamond head, and one arm per
        // diamond: 1 + 2 * n_diamonds nodes.
        assert_eq!(
            l.len(),
            2 * n_diamonds + 1,
            "each loop traverses stock, every diamond head, and one arm per diamond"
        );
    }
}

// --- Additional edge case tests ---

#[test]
fn test_self_loop_found() {
    // A self-loop (a -> a): check(a,1) sets visiting={a}, pushes a,
    // then explores edge a->a: check(a, score) finds a IS visiting
    // AND a=TARGET -> loop [a] is recorded.
    let graph = SearchGraph::from_edges(edges(&[("a", "a", 5.0)]), stock_list(&["a"]));

    let loops = graph.find_strongest_loops();
    assert_eq!(loops.len(), 1, "Self-loop should be found");
    assert_eq!(loops[0].len(), 1);
    assert_eq!(loops[0][0].as_str(), "a");
}

#[test]
fn test_two_separate_loops() {
    // Two disconnected loops: a<->b and c<->d. Each lives in its own SCC,
    // and each stock's search is confined to its own component, so both
    // are found independently.
    let graph = SearchGraph::from_edges(
        edges(&[
            ("a", "b", 1.0),
            ("b", "a", 1.0),
            ("c", "d", 1.0),
            ("d", "c", 1.0),
        ]),
        stock_list(&["a", "c"]),
    );

    let loops = graph.find_strongest_loops();
    assert_eq!(loops.len(), 2, "Should find two separate loops");
}

#[test]
fn test_stocks_without_outbound_edges() {
    // A stock that has no outbound edges shouldn't cause errors
    let graph = SearchGraph::from_edges(
        edges(&[("a", "b", 1.0), ("b", "a", 1.0)]),
        stock_list(&["a", "c"]), // c has no edges
    );

    let loops = graph.find_strongest_loops();
    assert_eq!(loops.len(), 1, "Should find the a-b loop, c is harmless");
}

#[test]
fn test_parse_link_offsets() {
    // Test the link offset parsing from variable names.
    // Use Ident::new() directly to match how the VM stores keys.
    let mut offsets = HashMap::new();
    offsets.insert(Ident::new("$⁚ltm⁚link_score⁚population→births"), 0usize);
    offsets.insert(Ident::new("$⁚ltm⁚link_score⁚births→population"), 1usize);
    offsets.insert(Ident::new("population"), 2usize);

    let results = Results {
        offsets,
        data: vec![0.0; 9].into_boxed_slice(),
        step_size: 3,
        step_count: 3,
        specs: crate::results::Specs {
            start: 0.0,
            stop: 2.0,
            dt: 1.0,
            save_step: 1.0,
            method: crate::results::Method::Euler,
            n_chunks: 3,
        },
        is_vensim: false,
    };

    let parsed = parse_link_offsets(&results, &[], &[]);
    assert_eq!(parsed.len(), 2, "Should find 2 link score variables");

    // Verify the parsed entries
    let has_pop_to_births = parsed
        .iter()
        .any(|((f, t), _)| f.as_str() == "population" && t.as_str() == "births");
    let has_births_to_pop = parsed
        .iter()
        .any(|((f, t), _)| f.as_str() == "births" && t.as_str() == "population");

    assert!(has_pop_to_births, "Should parse population->births link");
    assert!(has_births_to_pop, "Should parse births->population link");
}

#[test]
fn test_parse_link_offsets_a2a_expansion() {
    // An A2A link score `birth_rate->births` with dimension Region
    // (NYC, Boston, Chicago) should expand to 3 element-level entries.
    let mut offsets = HashMap::new();
    offsets.insert(Ident::new("$⁚ltm⁚link_score⁚birth_rate→births"), 10usize);
    // A scalar link score for comparison
    offsets.insert(Ident::new("$⁚ltm⁚link_score⁚scalar_a→scalar_b"), 20usize);

    let results = Results {
        offsets,
        data: vec![0.0; 30].into_boxed_slice(),
        step_size: 30,
        step_count: 1,
        specs: crate::results::Specs {
            start: 0.0,
            stop: 0.0,
            dt: 1.0,
            save_step: 1.0,
            method: crate::results::Method::Euler,
            n_chunks: 1,
        },
        is_vensim: false,
    };

    let ltm_vars = vec![
        crate::db::LtmSyntheticVar {
            name: "$\u{205A}ltm\u{205A}link_score\u{205A}birth_rate\u{2192}births".to_string(),
            equation: datamodel::Equation::Scalar(String::new()),
            dimensions: vec!["Region".to_string()],
            compile_directly: false,
        },
        crate::db::LtmSyntheticVar {
            name: "$\u{205A}ltm\u{205A}link_score\u{205A}scalar_a\u{2192}scalar_b".to_string(),
            equation: datamodel::Equation::Scalar(String::new()),
            dimensions: vec![],
            compile_directly: false,
        },
    ];
    let dims = vec![datamodel::Dimension::named(
        "Region".to_string(),
        vec![
            "NYC".to_string(),
            "Boston".to_string(),
            "Chicago".to_string(),
        ],
    )];

    let parsed = parse_link_offsets(&results, &ltm_vars, &dims);

    // Should have 3 element-level entries for A2A + 1 scalar = 4 total
    assert_eq!(parsed.len(), 4, "3 A2A elements + 1 scalar = 4 total");

    // Check A2A expansion: birth_rate[nyc]->births[nyc] at offset 10
    let nyc = parsed
        .iter()
        .find(|((f, t), _)| f.as_str() == "birth_rate[nyc]" && t.as_str() == "births[nyc]");
    assert!(nyc.is_some(), "Should have birth_rate[nyc]->births[nyc]");
    assert_eq!(nyc.unwrap().1, 10);

    let boston = parsed
        .iter()
        .find(|((f, t), _)| f.as_str() == "birth_rate[boston]" && t.as_str() == "births[boston]");
    assert!(
        boston.is_some(),
        "Should have birth_rate[boston]->births[boston]"
    );
    assert_eq!(boston.unwrap().1, 11);

    let chicago = parsed
        .iter()
        .find(|((f, t), _)| f.as_str() == "birth_rate[chicago]" && t.as_str() == "births[chicago]");
    assert!(
        chicago.is_some(),
        "Should have birth_rate[chicago]->births[chicago]"
    );
    assert_eq!(chicago.unwrap().1, 12);

    // Check scalar is unchanged
    let scalar = parsed
        .iter()
        .find(|((f, t), _)| f.as_str() == "scalar_a" && t.as_str() == "scalar_b");
    assert!(scalar.is_some(), "Scalar link should be preserved");
    assert_eq!(scalar.unwrap().1, 20);
}

#[test]
fn test_parse_link_offsets_cross_dim_passthrough() {
    // Cross-dimensional per-element scores (with `[` in the name)
    // should pass through directly without expansion.
    let mut offsets = HashMap::new();
    offsets.insert(
        Ident::new("$⁚ltm⁚link_score⁚population[nyc]→total_pop"),
        5usize,
    );

    let results = Results {
        offsets,
        data: vec![0.0; 10].into_boxed_slice(),
        step_size: 10,
        step_count: 1,
        specs: crate::results::Specs {
            start: 0.0,
            stop: 0.0,
            dt: 1.0,
            save_step: 1.0,
            method: crate::results::Method::Euler,
            n_chunks: 1,
        },
        is_vensim: false,
    };

    // Even with ltm_vars and dims, cross-dim scores pass through directly
    let parsed = parse_link_offsets(&results, &[], &[]);
    assert_eq!(parsed.len(), 1);
    let ((from, to), offset) = &parsed[0];
    assert_eq!(from.as_str(), "population[nyc]");
    assert_eq!(to.as_str(), "total_pop");
    assert_eq!(*offset, 5);
}

/// Helper: build a single-step Results object with the given offsets.
/// Tests in this module only care about the variable->offset mapping
/// (parse_link_offsets does not read data values), so the data buffer
/// is sized generously and zeroed.
fn make_results_with_offsets(
    offsets: HashMap<Ident<Canonical>, usize>,
    step_size: usize,
) -> Results {
    Results {
        offsets,
        data: vec![0.0; step_size].into_boxed_slice(),
        step_size,
        step_count: 1,
        specs: crate::results::Specs {
            start: 0.0,
            stop: 0.0,
            dt: 1.0,
            save_step: 1.0,
            method: crate::results::Method::Euler,
            n_chunks: 1,
        },
        is_vensim: false,
    }
}

/// Test 4: A FixedIndex A2A link score (`pop[nyc]→rel_pop` with
/// non-empty dimensions). The `from_str` already carries the source
/// element subscript; the per-slot expansion runs over the *target*
/// dimension. Each slot represents the link score for `(pop[nyc],
/// rel_pop[d])` at element `d`.
#[test]
fn test_parse_link_offsets_fixed_index_from_a2a_expansion() {
    let mut offsets = HashMap::new();
    offsets.insert(
        Ident::new("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}rel_pop"),
        100usize,
    );

    let results = make_results_with_offsets(offsets, 110);

    let ltm_vars = vec![crate::db::LtmSyntheticVar {
        name: "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}rel_pop".to_string(),
        equation: datamodel::Equation::Scalar(String::new()),
        dimensions: vec!["Region".to_string()],
        compile_directly: false,
    }];
    let dims = vec![datamodel::Dimension::named(
        "Region".to_string(),
        vec![
            "NYC".to_string(),
            "Boston".to_string(),
            "Chicago".to_string(),
        ],
    )];

    let parsed = parse_link_offsets(&results, &ltm_vars, &dims);

    assert_eq!(
        parsed.len(),
        3,
        "FixedIndex A2A should expand into one entry per target element"
    );

    // The from-name is fixed as `pop[nyc]` for all entries; only the
    // to-name varies per element, with the offset incrementing by 1.
    let nyc = parsed
        .iter()
        .find(|((f, t), _)| f.as_str() == "pop[nyc]" && t.as_str() == "rel_pop[nyc]");
    assert!(
        nyc.is_some(),
        "Should have pop[nyc]->rel_pop[nyc] at base offset"
    );
    assert_eq!(nyc.unwrap().1, 100);

    let boston = parsed
        .iter()
        .find(|((f, t), _)| f.as_str() == "pop[nyc]" && t.as_str() == "rel_pop[boston]");
    assert!(
        boston.is_some(),
        "Should have pop[nyc]->rel_pop[boston] at base+1"
    );
    assert_eq!(boston.unwrap().1, 101);

    let chicago = parsed
        .iter()
        .find(|((f, t), _)| f.as_str() == "pop[nyc]" && t.as_str() == "rel_pop[chicago]");
    assert!(
        chicago.is_some(),
        "Should have pop[nyc]->rel_pop[chicago] at base+2"
    );
    assert_eq!(chicago.unwrap().1, 102);
}

/// Test 5: A FixedIndex scalar link score (`pop[nyc]→total` with empty
/// dimensions) is element-level on the source side and scalar on the
/// target side. It should yield a single LinkOffset with no expansion.
#[test]
fn test_parse_link_offsets_fixed_index_from_scalar() {
    let mut offsets = HashMap::new();
    offsets.insert(
        Ident::new("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}total"),
        42usize,
    );

    let results = make_results_with_offsets(offsets, 50);

    let ltm_vars = vec![crate::db::LtmSyntheticVar {
        name: "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}total".to_string(),
        equation: datamodel::Equation::Scalar(String::new()),
        dimensions: vec![],
        compile_directly: false,
    }];

    let parsed = parse_link_offsets(&results, &ltm_vars, &[]);

    assert_eq!(
        parsed.len(),
        1,
        "FixedIndex scalar should produce a single LinkOffset"
    );
    let ((from, to), offset) = &parsed[0];
    assert_eq!(from.as_str(), "pop[nyc]");
    assert_eq!(to.as_str(), "total");
    assert_eq!(*offset, 42);
}

/// AC3.3: A scalar-source -> arrayed-target link score named
/// `$⁚ltm⁚link_score⁚total_pop→migration[nyc]` (one scalar
/// `LtmSyntheticVar` per target element, `dimensions: vec![]`) resolves
/// to the edge `(total_pop, migration[nyc])` -- the scalar source stays
/// unsubscripted and the element survives on the `to` side.
///
/// This is the discovery-side contract that `try_scalar_to_arrayed_link_scores`
/// relies on: the `[`-in-`to` single-passthrough branch (Branch 2 of
/// `parse_link_offsets`'s four-way dispatch) handles the new name shape
/// with no parser change, exactly as the source-subscripted mirror
/// (`test_parse_link_offsets_fixed_index_from_scalar`) does. Pre-fix,
/// these edges were named as Bare-A2A vars with `dimensions = [target_dims]`,
/// which `expand_a2a_link_offsets` mis-expanded by inventing a
/// `total_pop[nyc]` node that doesn't match the unsubscripted `total_pop`
/// node from the reducer edges -- making the loop unreachable.
#[test]
fn test_parse_link_offsets_scalar_to_arrayed() {
    let mut offsets = HashMap::new();
    offsets.insert(
        Ident::new("$\u{205A}ltm\u{205A}link_score\u{205A}total_pop\u{2192}migration[nyc]"),
        0usize,
    );

    let results = make_results_with_offsets(offsets, 10);

    // No `ltm_vars` entry needed: with empty `var_dims`, the `[`-in-`to`
    // passthrough branch fires regardless of the lookup result.
    let parsed = parse_link_offsets(&results, &[], &[]);

    assert_eq!(
        parsed.len(),
        1,
        "scalar-to-arrayed per-target-element link score should produce a single LinkOffset"
    );
    let ((from, to), offset) = &parsed[0];
    assert_eq!(
        from.as_str(),
        "total_pop",
        "the scalar source must stay unsubscripted"
    );
    assert_eq!(
        to.as_str(),
        "migration[nyc]",
        "the target element must survive on the `to` side"
    );
    assert_eq!(*offset, 0);
}

/// ltm-503-cross-element-agg.AC4.6 (discovery side): a partial-reduce
/// link score `$⁚ltm⁚link_score⁚matrix[a,x]→agg[a]` -- element-level on
/// *both* sides, `dimensions: vec![]` -- resolves to the single edge
/// `(matrix[a,x], agg[a])`. It rides the same `[`-in-`from`-or-`to`
/// single-passthrough branch (Branch 2) the full-reduce per-source-element
/// names already use; no parser change is needed. Crucially it must NOT
/// be broadcast over `D1` (which the alternative `dimensions = ["D1"]`
/// shape would route through `expand_fixed_from_a2a_link_offsets`).
#[test]
fn test_parse_link_offsets_partial_reduce_passthrough() {
    let mut offsets = HashMap::new();
    offsets.insert(
        Ident::new("$\u{205A}ltm\u{205A}link_score\u{205A}matrix[a,x]\u{2192}agg[a]"),
        0usize,
    );

    let results = make_results_with_offsets(offsets, 10);

    // No `ltm_vars` entry needed: with empty `var_dims`, the
    // element-level passthrough branch fires regardless of the lookup.
    let parsed = parse_link_offsets(&results, &[], &[]);

    assert_eq!(
        parsed.len(),
        1,
        "partial-reduce per-(d1,d2) link score should produce a single LinkOffset"
    );
    let ((from, to), offset) = &parsed[0];
    assert_eq!(
        from.as_str(),
        "matrix[a,x]",
        "the source subscript carries both the surviving and reduced axes"
    );
    assert_eq!(
        to.as_str(),
        "agg[a]",
        "the target subscript carries only the surviving axis"
    );
    assert_eq!(*offset, 0);
}

/// Regression test: when both a Bare A2A link score (`pop→share`)
/// and a FixedIndex A2A link score (`pop[nyc]→share`) exist for
/// the same edge -- e.g., `share[Region] = pop + pop[NYC]` -- both
/// expand to the per-element key `(pop[nyc], share[nyc])` at
/// different offsets. FixedIndex names carry the `FixedIndex` rank
/// (a bracketed `from`), so this collision is broken deterministically
/// in Bare's favor rather than left tied and resolved by HashMap
/// insertion order over `results.offsets`.
#[test]
fn test_parse_link_offsets_dedupes_a2a_bare_over_fixed_index() {
    let mut offsets = HashMap::new();
    offsets.insert(
        Ident::new("$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share"),
        10usize,
    );
    offsets.insert(
        Ident::new("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}share"),
        20usize,
    );

    let results = make_results_with_offsets(offsets, 30);

    let ltm_vars = vec![
        crate::db::LtmSyntheticVar {
            name: "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share".to_string(),
            equation: datamodel::Equation::Scalar(String::new()),
            dimensions: vec!["Region".to_string()],
            compile_directly: false,
        },
        crate::db::LtmSyntheticVar {
            name: "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}share".to_string(),
            equation: datamodel::Equation::Scalar(String::new()),
            dimensions: vec!["Region".to_string()],
            compile_directly: false,
        },
    ];
    let dims = vec![datamodel::Dimension::named(
        "Region".to_string(),
        vec!["NYC".to_string(), "Boston".to_string()],
    )];

    let parsed = parse_link_offsets(&results, &ltm_vars, &dims);

    // The aliased per-element key (pop[nyc], share[nyc]) appears
    // in both Bare A2A and FixedIndex A2A expansions; dedup must
    // pick Bare deterministically.
    let nyc_aliased: Vec<&LinkOffset> = parsed
        .iter()
        .filter(|((f, t), _)| f.as_str() == "pop[nyc]" && t.as_str() == "share[nyc]")
        .collect();
    assert_eq!(
        nyc_aliased.len(),
        1,
        "aliased per-element key (pop[nyc], share[nyc]) must dedupe to one entry; \
             got {} entries: {parsed:?}",
        nyc_aliased.len(),
    );
    assert_eq!(
        nyc_aliased[0].1, 10,
        "must pick Bare A2A's offset (10) over FixedIndex A2A's (20)",
    );

    // The non-aliased FixedIndex entry (pop[nyc], share[boston])
    // -- which Bare A2A doesn't produce -- must survive at
    // FixedIndex's offset.
    let boston_only_fixed: Vec<&LinkOffset> = parsed
        .iter()
        .filter(|((f, t), _)| f.as_str() == "pop[nyc]" && t.as_str() == "share[boston]")
        .collect();
    assert_eq!(
        boston_only_fixed.len(),
        1,
        "non-aliased FixedIndex entry (pop[nyc], share[boston]) must survive",
    );
    assert_eq!(
        boston_only_fixed[0].1, 21,
        "non-aliased FixedIndex entry must keep its offset (FixedIndex base 20 + boston index 1)",
    );
}

#[test]
fn test_assign_loop_ids() {
    let mut loops = vec![
        FoundLoop {
            loop_info: Loop {
                id: String::new(),
                links: vec![
                    Link {
                        from: Ident::new("x"),
                        to: Ident::new("y"),
                        polarity: crate::ltm::LinkPolarity::Positive,
                    },
                    Link {
                        from: Ident::new("y"),
                        to: Ident::new("x"),
                        polarity: crate::ltm::LinkPolarity::Positive,
                    },
                ],
                stocks: vec![],
                polarity: LoopPolarity::Reinforcing,
                dimensions: vec![],
                slot_links: vec![],
            },
            scores: vec![],
            avg_abs_score: 1.0,
            rel_scores: vec![],
            partition: None,
        },
        FoundLoop {
            loop_info: Loop {
                id: String::new(),
                links: vec![
                    Link {
                        from: Ident::new("a"),
                        to: Ident::new("b"),
                        polarity: crate::ltm::LinkPolarity::Negative,
                    },
                    Link {
                        from: Ident::new("b"),
                        to: Ident::new("a"),
                        polarity: crate::ltm::LinkPolarity::Positive,
                    },
                ],
                stocks: vec![],
                polarity: LoopPolarity::Balancing,
                dimensions: vec![],
                slot_links: vec![],
            },
            scores: vec![],
            avg_abs_score: 0.5,
            rel_scores: vec![],
            partition: None,
        },
    ];

    assign_loop_ids(&mut loops);

    // After sorting by content key, a_b comes before x_y
    let a_b_loop = loops
        .iter()
        .find(|l| {
            l.loop_info
                .links
                .iter()
                .any(|link| link.from.as_str() == "a")
        })
        .unwrap();
    let x_y_loop = loops
        .iter()
        .find(|l| {
            l.loop_info
                .links
                .iter()
                .any(|link| link.from.as_str() == "x")
        })
        .unwrap();

    assert_eq!(a_b_loop.loop_info.id, "b1");
    assert_eq!(x_y_loop.loop_info.id, "r1");
}

#[test]
fn test_assign_loop_ids_order_independent_for_sibling_cycles() {
    // GH #497, discovery-path twin of the structural-path test in
    // `ltm::tests`. Two sibling 3-cycles over {a,b,c} -- a->b->c->a and
    // a->c->b->a -- share a deduped variable set, so the primary sort key
    // ties them. Without the canonical-edge-sequence tiebreaker, the
    // stable-sort fallback leaks the (process-dependent) discovery-DFS
    // emission order into the assigned ids. Feed both input orderings and
    // assert each directed cycle keeps the same id.
    let forward = || {
        make_found_loop(
            &[("a", "b"), ("b", "c"), ("c", "a")],
            &[],
            LoopPolarity::Reinforcing,
            1.0,
        )
    };
    let reverse = || {
        make_found_loop(
            &[("a", "c"), ("c", "b"), ("b", "a")],
            &[],
            LoopPolarity::Reinforcing,
            1.0,
        )
    };
    // The directed cycle's identity is its canonical `link.from` rotation.
    let directed_key = |fl: &FoundLoop| -> Vec<String> {
        let seq: Vec<String> = fl
            .loop_info
            .links
            .iter()
            .map(|l| l.from.as_str().to_string())
            .collect();
        crate::ltm::canonical_rotation(&seq)
    };

    let mut order_a = vec![forward(), reverse()];
    let mut order_b = vec![reverse(), forward()];
    assign_loop_ids(&mut order_a);
    assign_loop_ids(&mut order_b);

    let id_for = |loops: &[FoundLoop], key: &[&str]| -> String {
        let want: Vec<String> = key.iter().map(|s| s.to_string()).collect();
        loops
            .iter()
            .find(|fl| directed_key(fl) == want)
            .map(|fl| fl.loop_info.id.clone())
            .unwrap()
    };
    assert_eq!(
        id_for(&order_a, &["a", "b", "c"]),
        id_for(&order_b, &["a", "b", "c"]),
        "forward sibling must get the same id regardless of input order"
    );
    assert_eq!(
        id_for(&order_a, &["a", "c", "b"]),
        id_for(&order_b, &["a", "c", "b"]),
        "reverse sibling must get the same id regardless of input order"
    );
    // And the two siblings must receive distinct ids (the tiebreaker
    // separates them rather than collapsing them).
    assert_ne!(
        id_for(&order_a, &["a", "b", "c"]),
        id_for(&order_a, &["a", "c", "b"]),
        "the two siblings must receive distinct ids"
    );
}

/// Helper to create a FoundLoop with given variable names, polarity, and score.
/// Populates a single timestep of score data so per-timestep filtering works.
fn make_found_loop(
    var_pairs: &[(&str, &str)],
    stocks: &[&str],
    polarity: LoopPolarity,
    avg_abs_score: f64,
) -> FoundLoop {
    make_found_loop_with_scores(
        var_pairs,
        stocks,
        polarity,
        avg_abs_score,
        vec![(0.0, avg_abs_score)],
    )
}

fn make_found_loop_with_scores(
    var_pairs: &[(&str, &str)],
    stocks: &[&str],
    polarity: LoopPolarity,
    avg_abs_score: f64,
    scores: Vec<(f64, f64)>,
) -> FoundLoop {
    let links: Vec<Link> = var_pairs
        .iter()
        .map(|(from, to)| Link {
            from: Ident::new(from),
            to: Ident::new(to),
            polarity: crate::ltm::LinkPolarity::Positive,
        })
        .collect();
    FoundLoop {
        loop_info: Loop {
            id: String::new(),
            links,
            stocks: stocks.iter().map(|s| Ident::new(s)).collect(),
            polarity,
            dimensions: vec![],
            slot_links: vec![],
        },
        scores,
        avg_abs_score,
        rel_scores: vec![],
        partition: None,
    }
}

/// Create a CyclePartitions where all given stocks are in a single partition.
fn single_partition(stocks: &[&str]) -> CyclePartitions {
    let stock_idents: Vec<Ident<Canonical>> = stocks.iter().map(|s| Ident::new(s)).collect();
    let stock_partition: HashMap<Ident<Canonical>, usize> =
        stock_idents.iter().map(|s| (s.clone(), 0)).collect();
    CyclePartitions {
        partitions: vec![stock_idents],
        stock_partition,
    }
}

#[test]
fn test_rank_and_filter_truncates_to_max_loops() {
    // Exercise the global cap with a test-only override and a tiny fixture
    // (per docs/dev/rust.md#test-time-budgets) rather than building 200+
    // loops to trip the production MAX_LOOPS constant.
    const CAP: usize = 3;
    const EXCESS: usize = 2;
    let stock_names: Vec<String> = (0..CAP + EXCESS).map(|i| format!("stock_{i:04}")).collect();
    let mut loops: Vec<FoundLoop> = (0..CAP + EXCESS)
        .map(|i| {
            let name_a = format!("var_a_{i:04}");
            let name_b = format!("var_b_{i:04}");
            make_found_loop(
                &[(&name_a, &name_b), (&name_b, &name_a)],
                &[&stock_names[i]],
                LoopPolarity::Reinforcing,
                // Give all loops equal score so none are filtered by MIN_CONTRIBUTION
                1.0,
            )
        })
        .collect();

    // All stocks in one partition so filtering works like before
    let all_stocks: Vec<&str> = stock_names.iter().map(|s| s.as_str()).collect();
    let partitions = single_partition(&all_stocks);

    assert_eq!(loops.len(), CAP + EXCESS);
    let _guard = MaxLoopsGuard::new(CAP);
    rank_and_filter(&mut loops, &partitions);
    assert_eq!(loops.len(), CAP, "Should truncate to the cap ({CAP})");
}

#[test]
fn test_rank_and_filter_removes_low_contribution() {
    // Create loops where one dominates and others have negligible contribution.
    // The dominant loop has score 1000; the tiny loop has score 0.0001.
    // Total = 1000.0001, tiny/total ~= 0.0000001 < MIN_CONTRIBUTION (0.001).
    let mut loops = vec![
        make_found_loop(
            &[("big_a", "big_b"), ("big_b", "big_a")],
            &["stock_x"],
            LoopPolarity::Reinforcing,
            1000.0,
        ),
        make_found_loop(
            &[("tiny_a", "tiny_b"), ("tiny_b", "tiny_a")],
            &["stock_x"],
            LoopPolarity::Balancing,
            0.0001,
        ),
    ];

    let partitions = single_partition(&["stock_x"]);
    rank_and_filter(&mut loops, &partitions);

    // Only the dominant loop should remain
    assert_eq!(
        loops.len(),
        1,
        "Loops below MIN_CONTRIBUTION should be filtered out"
    );
    assert_eq!(loops[0].avg_abs_score, 1000.0);
}

#[test]
fn test_rank_and_filter_preserves_score_ordering() {
    let mut loops = vec![
        make_found_loop(
            &[("low_a", "low_b"), ("low_b", "low_a")],
            &["stock_x"],
            LoopPolarity::Balancing,
            1.0,
        ),
        make_found_loop(
            &[("high_a", "high_b"), ("high_b", "high_a")],
            &["stock_x"],
            LoopPolarity::Reinforcing,
            100.0,
        ),
        make_found_loop(
            &[("mid_a", "mid_b"), ("mid_b", "mid_a")],
            &["stock_x"],
            LoopPolarity::Reinforcing,
            50.0,
        ),
    ];

    let partitions = single_partition(&["stock_x"]);
    rank_and_filter(&mut loops, &partitions);

    // Within a SINGLE partition the relative-contribution ranking (GH #543)
    // and the raw-magnitude ranking coincide (the same denominator divides
    // every loop), so the descending-magnitude order still holds here.
    assert_eq!(loops.len(), 3);
    assert_eq!(loops[0].avg_abs_score, 100.0);
    assert_eq!(loops[1].avg_abs_score, 50.0);
    assert_eq!(loops[2].avg_abs_score, 1.0);

    // IDs should be assigned (deterministically by content, but present)
    assert!(!loops[0].loop_info.id.is_empty());
    assert!(!loops[1].loop_info.id.is_empty());
    assert!(!loops[2].loop_info.id.is_empty());
}

#[test]
fn test_rank_and_filter_retains_briefly_dominant_loop() {
    // A loop that is dominant at 1 out of 100 timesteps (strong spike) but
    // has tiny average should be retained by per-timestep filtering.
    let n = 100;

    // Build score vectors: "spike" loop has score 100 at step 50, 0 elsewhere
    let spike_scores: Vec<(f64, f64)> = (0..n)
        .map(|i| {
            let t = i as f64;
            if i == 50 { (t, 100.0) } else { (t, 0.0) }
        })
        .collect();
    // avg_abs_score = 100/100 = 1.0
    let spike_loop = make_found_loop_with_scores(
        &[("spike_a", "spike_b"), ("spike_b", "spike_a")],
        &["stock_x"],
        LoopPolarity::Reinforcing,
        1.0,
        spike_scores,
    );

    // "steady" loop has score 50 at every step
    let steady_scores: Vec<(f64, f64)> = (0..n).map(|i| (i as f64, 50.0)).collect();
    let steady_loop = make_found_loop_with_scores(
        &[("steady_a", "steady_b"), ("steady_b", "steady_a")],
        &["stock_x"],
        LoopPolarity::Reinforcing,
        50.0,
        steady_scores,
    );

    let partitions = single_partition(&["stock_x"]);
    let mut loops = vec![spike_loop, steady_loop];
    rank_and_filter(&mut loops, &partitions);

    // Both loops should be retained: the spike loop has 100/(100+50) = 66.7%
    // contribution at step 50, well above MIN_CONTRIBUTION.
    assert_eq!(
        loops.len(),
        2,
        "Briefly dominant loop should be retained by per-timestep filtering"
    );
}

#[test]
fn test_rank_and_filter_partitioned_filtering() {
    // Two partitions: partition A has a dominant loop and a tiny loop.
    // Partition B has a single loop that would be globally negligible
    // but is the ONLY loop in its partition.
    //
    // Without partition-aware filtering, loop_b would be filtered out
    // because its score is tiny relative to the global total.
    // With partition-aware filtering, it's retained because it's 100%
    // of its partition's total.
    let mut loops = vec![
        make_found_loop(
            &[("big_a", "big_b"), ("big_b", "big_a")],
            &["stock_a"],
            LoopPolarity::Reinforcing,
            1000.0,
        ),
        make_found_loop(
            &[("small_a", "small_b"), ("small_b", "small_a")],
            &["stock_a"],
            LoopPolarity::Balancing,
            100.0,
        ),
        make_found_loop(
            &[("other_a", "other_b"), ("other_b", "other_a")],
            &["stock_x"],
            LoopPolarity::Reinforcing,
            0.01,
        ),
    ];

    let partitions = CyclePartitions {
        partitions: vec![vec![Ident::new("stock_a")], vec![Ident::new("stock_x")]],
        stock_partition: vec![(Ident::new("stock_a"), 0), (Ident::new("stock_x"), 1)]
            .into_iter()
            .collect(),
    };

    rank_and_filter(&mut loops, &partitions);

    // All 3 loops should be retained: the "other" loop is 100% of
    // its own partition's total at its timestep
    assert_eq!(
        loops.len(),
        3,
        "Loop dominant in its own partition should be retained even if globally tiny"
    );
}

/// AC7.5: SearchGraph built from element-level LinkOffset entries reads the
/// correct weight value from the correct result slot for each element.
///
/// A2A expansion maps `birth_rate→births` (with dimension Region = [nyc,
/// boston, chicago]) to three element-level `LinkOffset` entries:
///   `birth_rate[nyc]→births[nyc]`        at base_offset
///   `birth_rate[boston]→births[boston]`  at base_offset + 1
///   `birth_rate[chicago]→births[chicago]` at base_offset + 2
///
/// This test verifies that `SearchGraph::from_results` reads the value
/// stored at `base_offset + element_index` for each element-level edge,
/// not the value at `base_offset` for all of them. If the offset mapping
/// were wrong, each edge would carry the same weight (the value at
/// `base_offset`), and the assertions on per-element weights would fail.
#[test]
fn test_search_graph_from_results_element_level_weights() {
    let base_offset = 10usize;

    // Build a Results object: step_size large enough to hold all offsets.
    // One timestep (step=0); distinct values at base_offset/+1/+2 so we
    // can confirm each element-level edge reads its own result slot.
    //   nyc=0.8, boston=0.3, chicago=0.5
    let step_size = 20;
    let step_count = 1;
    let mut data = vec![0.0f64; step_size * step_count];
    data[base_offset] = 0.8; // birth_rate[nyc]    -> births[nyc]    (element 0)
    data[base_offset + 1] = 0.3; // birth_rate[boston] -> births[boston] (element 1)
    data[base_offset + 2] = 0.5; // birth_rate[chicago]-> births[chicago](element 2)

    let results = Results {
        offsets: HashMap::new(), // from_results does not use offsets
        data: data.into_boxed_slice(),
        step_size,
        step_count,
        specs: crate::results::Specs {
            start: 0.0,
            stop: 0.0,
            dt: 1.0,
            save_step: 1.0,
            method: crate::results::Method::Euler,
            n_chunks: 1,
        },
        is_vensim: false,
    };

    // Element-level LinkOffset entries produced by expand_a2a_link_offsets
    // for an A2A link score with three dimension elements.
    let link_offsets: Vec<LinkOffset> = vec![
        (
            (Ident::new("birth_rate[nyc]"), Ident::new("births[nyc]")),
            base_offset,
        ),
        (
            (
                Ident::new("birth_rate[boston]"),
                Ident::new("births[boston]"),
            ),
            base_offset + 1,
        ),
        (
            (
                Ident::new("birth_rate[chicago]"),
                Ident::new("births[chicago]"),
            ),
            base_offset + 2,
        ),
    ];

    let stocks = vec![Ident::new("population[nyc]")];
    let graph = SearchGraph::from_results(&results, 0, &link_offsets, &stocks);

    // Each element-level edge must carry the value stored at its own slot.
    // The SearchGraph adjacency list is keyed by the canonical "from" ident.
    let nyc_key = canonicalize("birth_rate[nyc]");
    let boston_key = canonicalize("birth_rate[boston]");
    let chicago_key = canonicalize("birth_rate[chicago]");

    let nyc_edges = graph.adj.get(&*nyc_key);
    assert!(
        nyc_edges.is_some(),
        "birth_rate[nyc] should have an outbound edge"
    );
    let nyc_score = nyc_edges.unwrap()[0].score;
    assert!(
        (nyc_score - 0.8).abs() < 1e-10,
        "birth_rate[nyc]->births[nyc] should have weight 0.8 (slot base_offset), got {nyc_score}"
    );

    let boston_edges = graph.adj.get(&*boston_key);
    assert!(
        boston_edges.is_some(),
        "birth_rate[boston] should have an outbound edge"
    );
    let boston_score = boston_edges.unwrap()[0].score;
    assert!(
        (boston_score - 0.3).abs() < 1e-10,
        "birth_rate[boston]->births[boston] should have weight 0.3 (slot base+1), got {boston_score}"
    );

    let chicago_edges = graph.adj.get(&*chicago_key);
    assert!(
        chicago_edges.is_some(),
        "birth_rate[chicago] should have an outbound edge"
    );
    let chicago_score = chicago_edges.unwrap()[0].score;
    assert!(
        (chicago_score - 0.5).abs() < 1e-10,
        "birth_rate[chicago]->births[chicago] should have weight 0.5 (slot base+2), got {chicago_score}"
    );

    // If all offsets pointed to base_offset+0 (wrong), all weights would
    // be 0.8. Distinct values (0.8, 0.3, 0.5) make this bug visible.
    assert!(
        (nyc_score - boston_score).abs() > 1e-10,
        "nyc and boston weights must differ; both being {nyc_score} indicates an offset bug"
    );
    assert!(
        (boston_score - chicago_score).abs() > 1e-10,
        "boston and chicago weights must differ; both being {boston_score} indicates an offset bug"
    );
}

#[test]
fn test_rank_and_filter_element_level_partitions() {
    // Element-level partitions: population[nyc] and population[boston]
    // are separate stocks in the same partition. A tiny loop through
    // population[chicago] in a separate partition should be retained
    // because it dominates its own partition.
    let mut loops = vec![
        make_found_loop(
            &[
                ("population[nyc]", "births[nyc]"),
                ("births[nyc]", "population[nyc]"),
            ],
            &["population[nyc]"],
            LoopPolarity::Reinforcing,
            500.0,
        ),
        make_found_loop(
            &[
                ("population[boston]", "births[boston]"),
                ("births[boston]", "population[boston]"),
            ],
            &["population[boston]"],
            LoopPolarity::Reinforcing,
            400.0,
        ),
        make_found_loop(
            &[
                ("population[chicago]", "births[chicago]"),
                ("births[chicago]", "population[chicago]"),
            ],
            &["population[chicago]"],
            LoopPolarity::Reinforcing,
            0.01,
        ),
    ];

    // Two partitions: NYC+Boston share a partition (connected by
    // some cross-element feedback), Chicago is alone.
    let partitions = CyclePartitions {
        partitions: vec![
            vec![
                Ident::new("population[boston]"),
                Ident::new("population[nyc]"),
            ],
            vec![Ident::new("population[chicago]")],
        ],
        stock_partition: vec![
            (Ident::new("population[nyc]"), 0),
            (Ident::new("population[boston]"), 0),
            (Ident::new("population[chicago]"), 1),
        ]
        .into_iter()
        .collect(),
    };

    let partition_meta = rank_and_filter(&mut loops, &partitions);

    // All 3 loops should be retained: Chicago's loop is 100% of its
    // partition's total, even though globally it's tiny.
    assert_eq!(
        loops.len(),
        3,
        "Element-level loop dominant in its partition should be retained"
    );

    // Ordering is partition-RELATIVE among competing loops (GH #543),
    // with trivially-isolated loops demoted below them.  NYC
    // (500/(500+400) = 0.556) ranks above Boston (400/900 = 0.444); the
    // Chicago loop is ALONE in its partition, so its 1.0 relative score
    // is degenerate (±1 by construction) and it sorts after the
    // competing pair despite the larger mean-rel.
    assert_eq!(loops[0].avg_abs_score, 500.0);
    assert_eq!(loops[1].avg_abs_score, 400.0);
    assert!(
        (loops[2].avg_abs_score - 0.01).abs() < 1e-10,
        "Chicago (solo-partition, rel 1.0 by construction) ranks last; got {}",
        loops[2].avg_abs_score
    );

    // Partition metadata: dense, first-appearance order. Partition 0 is
    // the NYC/Boston SCC (two element-level stocks, two returned loops);
    // partition 1 is Chicago's singleton.
    assert_eq!(partition_meta.len(), 2);
    assert_eq!(
        partition_meta[0].stocks,
        vec![
            "population[boston]".to_string(),
            "population[nyc]".to_string()
        ]
    );
    assert_eq!(partition_meta[0].loop_count, 2);
    assert_eq!(
        partition_meta[1].stocks,
        vec!["population[chicago]".to_string()]
    );
    assert_eq!(partition_meta[1].loop_count, 1);
    assert_eq!(loops[0].partition, Some(0));
    assert_eq!(loops[1].partition, Some(0));
    assert_eq!(loops[2].partition, Some(1));
}

/// Build a two-partition CyclePartitions where each partition holds the
/// listed stocks. `a_stocks` -> partition 0, `b_stocks` -> partition 1.
fn two_partitions(a_stocks: &[&str], b_stocks: &[&str]) -> CyclePartitions {
    let a: Vec<Ident<Canonical>> = a_stocks.iter().map(|s| Ident::new(s)).collect();
    let b: Vec<Ident<Canonical>> = b_stocks.iter().map(|s| Ident::new(s)).collect();
    let mut stock_partition: HashMap<Ident<Canonical>, usize> = HashMap::new();
    for s in &a {
        stock_partition.insert(s.clone(), 0);
    }
    for s in &b {
        stock_partition.insert(s.clone(), 1);
    }
    CyclePartitions {
        partitions: vec![a, b],
        stock_partition,
    }
}

/// GH #543: ranking must be partition-RELATIVE, not raw magnitude.
///
/// Partition A is high-magnitude and holds a dominant loop (a_big, rel
/// 0.7) and a non-dominant loop (a_small, rel 0.3). Partition B is
/// low-magnitude and holds a dominant loop (b_dom, rel ~0.91) plus a
/// minor one (b_min, rel ~0.09). The partition-B-dominant loop must rank
/// ABOVE both partition-A loops even though its raw magnitude (0.5) is
/// ~1000x smaller -- the relative key, not the raw `avg_abs_score`,
/// drives the order.
#[test]
fn test_rank_and_filter_543_partition_relative_ranking() {
    // Partition A: a_big = 700, a_small = 300 -> rels 0.7 and 0.3.
    // Partition B: b_dom = 0.5, b_min = 0.05 -> rels ~0.909 and ~0.091.
    let mut loops = vec![
        make_found_loop(
            &[("a_big_x", "a_big_y"), ("a_big_y", "a_big_x")],
            &["stock_a"],
            LoopPolarity::Reinforcing,
            700.0,
        ),
        make_found_loop(
            &[("a_small_x", "a_small_y"), ("a_small_y", "a_small_x")],
            &["stock_a"],
            LoopPolarity::Reinforcing,
            300.0,
        ),
        make_found_loop(
            &[("b_dom_x", "b_dom_y"), ("b_dom_y", "b_dom_x")],
            &["stock_b"],
            LoopPolarity::Reinforcing,
            0.5,
        ),
        make_found_loop(
            &[("b_min_x", "b_min_y"), ("b_min_y", "b_min_x")],
            &["stock_b"],
            LoopPolarity::Reinforcing,
            0.05,
        ),
    ];

    let partitions = two_partitions(&["stock_a"], &["stock_b"]);
    rank_and_filter(&mut loops, &partitions);

    let order: Vec<f64> = loops.iter().map(|l| l.avg_abs_score).collect();
    // Relative ranking: b_dom (0.909) > a_big (0.7) > a_small (0.3) >
    // b_min (0.091).
    assert_eq!(loops.len(), 4, "all four loops clear MIN_CONTRIBUTION");
    assert_eq!(
        order[0], 0.5,
        "partition-B-dominant loop (rel ~0.91) must rank first, not the high-magnitude loops"
    );
    assert_eq!(order[1], 700.0, "a_big (rel 0.7) is second");
    assert_eq!(order[2], 300.0, "a_small (rel 0.3) is third");
    assert_eq!(order[3], 0.05, "b_min (rel ~0.09) is last");
}

/// A loop trivially ALONE in its cycle partition has relative score
/// exactly ±1 at every active step by construction -- zero discriminative
/// information -- so it must sort AFTER every competing loop, regardless
/// of the competing loops' (necessarily smaller) shares.  This is the
/// C-LEARN failure mode: dozens of isolated two-variable gas-uptake loops
/// pinned the top of the discovery ranking above the carbon-climate core
/// where loops genuinely compete.
#[test]
fn test_rank_and_filter_demotes_trivially_isolated_loops() {
    let mut loops = vec![
        // The "core": two competing loops in partition A.
        make_found_loop(
            &[("a_big_x", "a_big_y"), ("a_big_y", "a_big_x")],
            &["stock_a"],
            LoopPolarity::Reinforcing,
            700.0,
        ),
        make_found_loop(
            &[("a_small_x", "a_small_y"), ("a_small_y", "a_small_x")],
            &["stock_a"],
            LoopPolarity::Balancing,
            300.0,
        ),
        // A trivially-isolated stock-decay loop: alone in partition B,
        // rel 1.0 by construction.
        make_found_loop(
            &[("b_only_x", "b_only_y"), ("b_only_y", "b_only_x")],
            &["stock_b"],
            LoopPolarity::Reinforcing,
            1.0,
        ),
    ];

    let partitions = two_partitions(&["stock_a"], &["stock_b"]);
    let partition_meta = rank_and_filter(&mut loops, &partitions);

    assert_eq!(loops.len(), 3, "all three loops clear MIN_CONTRIBUTION");
    let order: Vec<f64> = loops.iter().map(|l| l.avg_abs_score).collect();
    assert_eq!(
        order,
        vec![700.0, 300.0, 1.0],
        "competing loops rank by share; the solo-partition loop sorts after ALL of them"
    );

    // Partition metadata reflects the final order: partition 0 is the
    // competitive one (first appearance), partition 1 the singleton.
    assert_eq!(partition_meta.len(), 2);
    assert_eq!(partition_meta[0].stocks, vec!["stock_a".to_string()]);
    assert_eq!(partition_meta[0].loop_count, 2);
    assert_eq!(partition_meta[1].stocks, vec!["stock_b".to_string()]);
    assert_eq!(partition_meta[1].loop_count, 1);
    assert_eq!(
        loops.iter().map(|l| l.partition).collect::<Vec<_>>(),
        vec![Some(0), Some(0), Some(1)]
    );
}

/// GH #543 (truncation arm): under a small cap, the partition-dominant
/// low-magnitude loop must be RETAINED over the higher-magnitude
/// non-dominant loop in a busier partition. RED against the old code,
/// which truncated by raw `avg_abs_score` and would keep a_small (300)
/// while dropping b_dom (0.5).
#[test]
fn test_rank_and_filter_543_truncation_keeps_partition_dominant() {
    let mut loops = vec![
        make_found_loop(
            &[("a_big_x", "a_big_y"), ("a_big_y", "a_big_x")],
            &["stock_a"],
            LoopPolarity::Reinforcing,
            700.0,
        ),
        make_found_loop(
            &[("a_small_x", "a_small_y"), ("a_small_y", "a_small_x")],
            &["stock_a"],
            LoopPolarity::Reinforcing,
            300.0,
        ),
        make_found_loop(
            &[("b_dom_x", "b_dom_y"), ("b_dom_y", "b_dom_x")],
            &["stock_b"],
            LoopPolarity::Reinforcing,
            0.5,
        ),
        make_found_loop(
            &[("b_min_x", "b_min_y"), ("b_min_y", "b_min_x")],
            &["stock_b"],
            LoopPolarity::Reinforcing,
            0.05,
        ),
    ];

    let partitions = two_partitions(&["stock_a"], &["stock_b"]);
    // Test-only cap of 2: only the two highest relative-importance loops
    // survive. Those are b_dom (rel ~0.91) and a_big (rel 0.7); a_small
    // (rel 0.3) and b_min (rel ~0.09) are dropped. Under the OLD
    // raw-magnitude truncation the survivors would have been a_big (700)
    // and a_small (300), dropping the partition-dominant b_dom.
    let _guard = MaxLoopsGuard::new(2);
    rank_and_filter(&mut loops, &partitions);

    assert_eq!(loops.len(), 2, "cap of 2 retains exactly two loops");
    let mags: Vec<f64> = loops.iter().map(|l| l.avg_abs_score).collect();
    assert!(
        mags.contains(&0.5),
        "partition-dominant low-magnitude loop must survive the cap (GH #543); got {mags:?}"
    );
    assert!(
        !mags.contains(&300.0),
        "the high-magnitude non-dominant loop must be dropped under the relative cap; got {mags:?}"
    );
}

/// Under cap pressure, trivially-isolated (solo-partition) loops are
/// dropped BEFORE any competing loop -- they are the zero-information
/// entries.  Among the solos, the content key breaks the tie
/// deterministically.
#[test]
fn test_rank_and_filter_truncation_drops_solo_loops_first() {
    let mut loops = vec![
        // Two competing loops in partition A with small shares.
        make_found_loop(
            &[("a_big_x", "a_big_y"), ("a_big_y", "a_big_x")],
            &["stock_a"],
            LoopPolarity::Reinforcing,
            7.0,
        ),
        make_found_loop(
            &[("a_small_x", "a_small_y"), ("a_small_y", "a_small_x")],
            &["stock_a"],
            LoopPolarity::Reinforcing,
            3.0,
        ),
        // Two solo loops, each alone in its own partition (rel 1.0 each).
        make_found_loop(
            &[("b_only_x", "b_only_y"), ("b_only_y", "b_only_x")],
            &["stock_b"],
            LoopPolarity::Reinforcing,
            100.0,
        ),
        make_found_loop(
            &[("c_only_x", "c_only_y"), ("c_only_y", "c_only_x")],
            &["stock_c"],
            LoopPolarity::Reinforcing,
            100.0,
        ),
    ];

    let stock_b = Ident::new("stock_b");
    let stock_c = Ident::new("stock_c");
    let stock_a = Ident::new("stock_a");
    let partitions = CyclePartitions {
        partitions: vec![
            vec![stock_a.clone()],
            vec![stock_b.clone()],
            vec![stock_c.clone()],
        ],
        stock_partition: vec![(stock_a, 0), (stock_b, 1), (stock_c, 2)]
            .into_iter()
            .collect(),
    };

    // Cap of 3: both competing loops survive (rel 0.7 and 0.3) plus ONE
    // solo loop (content-key tiebreak picks b before c); the other solo
    // is dropped even though its raw magnitude (100) dwarfs the
    // competing loops'.
    let _guard = MaxLoopsGuard::new(3);
    let partition_meta = rank_and_filter(&mut loops, &partitions);

    assert_eq!(loops.len(), 3, "cap of 3 retains exactly three loops");
    let order: Vec<f64> = loops.iter().map(|l| l.avg_abs_score).collect();
    assert_eq!(order[0], 7.0, "competing loops first");
    assert_eq!(order[1], 3.0);
    assert_eq!(order[2], 100.0, "one solo loop fills the last slot");
    assert!(
        loops[2].loop_info.links[0].from.as_str().starts_with("b_"),
        "content-key tiebreak among equal solos must pick b_only deterministically"
    );
    // Only the partitions of RETURNED loops appear in the metadata.
    assert_eq!(partition_meta.len(), 2);
    assert_eq!(partition_meta[1].stocks, vec!["stock_b".to_string()]);
}

/// GH #310: a partition-dominant loop globally ranked BELOW the cap must
/// survive, because the partition-aware retention filter runs before the
/// global truncation. RED against the old truncate-before-filter order.
///
/// Build several high-magnitude loops in partition A plus a tiny
/// partition-B pair whose dominant loop has globally negligible
/// magnitude. With a tiny cap and the OLD order (truncate-by-magnitude
/// THEN filter), the partition-B-dominant loop -- globally among the
/// lowest magnitudes -- is truncated away before the partition scope ever
/// sees it. With the new order it is retained: it is ~91% of its
/// partition and the relative ranking floats it to the top.
#[test]
fn test_rank_and_filter_310_partition_dominant_survives_cap() {
    let mut loops = vec![
        make_found_loop(
            &[("a1x", "a1y"), ("a1y", "a1x")],
            &["stock_a"],
            LoopPolarity::Reinforcing,
            900.0,
        ),
        make_found_loop(
            &[("a2x", "a2y"), ("a2y", "a2x")],
            &["stock_a"],
            LoopPolarity::Reinforcing,
            800.0,
        ),
        make_found_loop(
            &[("a3x", "a3y"), ("a3y", "a3x")],
            &["stock_a"],
            LoopPolarity::Reinforcing,
            700.0,
        ),
        // Globally tiny magnitudes, but b_dom dominates partition B.
        make_found_loop(
            &[("b_dom_x", "b_dom_y"), ("b_dom_y", "b_dom_x")],
            &["stock_b"],
            LoopPolarity::Reinforcing,
            0.5,
        ),
        make_found_loop(
            &[("b_min_x", "b_min_y"), ("b_min_y", "b_min_x")],
            &["stock_b"],
            LoopPolarity::Reinforcing,
            0.05,
        ),
    ];

    let partitions = two_partitions(&["stock_a"], &["stock_b"]);
    // Cap of 1: only the single most partition-relatively-important loop
    // survives. That is the partition-B-dominant loop (rel ~0.91), NOT
    // any partition-A loop (a1's rel is 900/2400 = 0.375). Under the OLD
    // truncate-before-filter the survivor would have been a1 (magnitude
    // 900) and the partition-B loop would never have been seen.
    let _guard = MaxLoopsGuard::new(1);
    rank_and_filter(&mut loops, &partitions);

    assert_eq!(loops.len(), 1, "cap of 1 retains exactly one loop");
    assert_eq!(
        loops[0].avg_abs_score, 0.5,
        "the partition-dominant loop (globally below the cap) must survive (GH #310)"
    );
}

/// Determinism: the retained set, assigned IDs, and final ordering must be
/// invariant under input permutation. Feeds the #543 fixture in two
/// different input orders and asserts byte-identical results.
#[test]
fn test_rank_and_filter_deterministic_under_permutation() {
    let build = || {
        vec![
            make_found_loop(
                &[("a_big_x", "a_big_y"), ("a_big_y", "a_big_x")],
                &["stock_a"],
                LoopPolarity::Reinforcing,
                700.0,
            ),
            make_found_loop(
                &[("a_small_x", "a_small_y"), ("a_small_y", "a_small_x")],
                &["stock_a"],
                LoopPolarity::Balancing,
                300.0,
            ),
            make_found_loop(
                &[("b_only_x", "b_only_y"), ("b_only_y", "b_only_x")],
                &["stock_b"],
                LoopPolarity::Reinforcing,
                1.0,
            ),
        ]
    };
    let partitions = two_partitions(&["stock_a"], &["stock_b"]);

    let mut order_a = build();
    let mut order_b = build();
    order_b.reverse();

    rank_and_filter(&mut order_a, &partitions);
    rank_and_filter(&mut order_b, &partitions);

    // Same final ordering (by magnitude proxy), same ids, same partition
    // assignment, same retained set.
    let proj = |loops: &[FoundLoop]| -> Vec<(f64, String, Option<usize>)> {
        loops
            .iter()
            .map(|l| (l.avg_abs_score, l.loop_info.id.clone(), l.partition))
            .collect()
    };
    assert_eq!(
        proj(&order_a),
        proj(&order_b),
        "permuted input must yield identical ordering, ids, and partitions"
    );
}

/// The no-score-data path (zero timesteps) still attaches partition
/// metadata: partitions are structural, not score-derived.
#[test]
fn test_rank_and_filter_no_scores_still_attaches_partitions() {
    let mut loops = vec![
        make_found_loop_with_scores(
            &[("ax", "ay"), ("ay", "ax")],
            &["stock_a"],
            LoopPolarity::Reinforcing,
            0.0,
            vec![],
        ),
        make_found_loop_with_scores(
            &[("bx", "by"), ("by", "bx")],
            &["stock_b"],
            LoopPolarity::Reinforcing,
            0.0,
            vec![],
        ),
    ];

    let partitions = two_partitions(&["stock_a"], &["stock_b"]);
    let partition_meta = rank_and_filter(&mut loops, &partitions);

    assert_eq!(loops.len(), 2);
    assert_eq!(partition_meta.len(), 2);
    assert!(
        loops.iter().all(|l| l.partition.is_some()),
        "every loop must carry its partition even with no score data"
    );
    // Dense first-appearance indexing holds on this path too.
    assert_eq!(loops[0].partition, Some(0));
    assert_eq!(loops[1].partition, Some(1));
    assert_eq!(partition_meta[0].loop_count, 1);
    assert_eq!(partition_meta[1].loop_count, 1);
}

// --- IndexedSearch vs. SearchGraph equivalence oracle ---
//
// `discover_loops_with_graph` was optimized from a per-timestep
// `SearchGraph` rebuild (Ident-keyed HashMaps, full-string hashing in the
// DFS) to a once-built `IndexedSearch` over dense integer ids. The two
// must discover *exactly* the same loop paths in the same first-seen order.
// These tests lock that equivalence in by running both paths over a range
// of synthetic graphs and comparing the resulting `all_paths` verbatim.

/// The original cross-step discovery loop, reproduced over the retained
/// `SearchGraph` reference implementation. Returns the deduped `all_paths`
/// in first-seen order, exactly as the pre-optimization
/// `discover_loops_with_graph` body did.
fn reference_all_paths(
    results: &Results,
    link_offsets: &[LinkOffset],
    stocks: &[Ident<Canonical>],
) -> Vec<Vec<Ident<Canonical>>> {
    let mut all_paths: Vec<Vec<Ident<Canonical>>> = Vec::new();
    let mut seen_sets: HashSet<Vec<String>> = HashSet::new();
    for step in 1..results.step_count {
        let graph = SearchGraph::from_results(results, step, link_offsets, stocks);
        for path in graph.find_strongest_loops() {
            let path_strings: Vec<String> = path.iter().map(|id| id.as_str().to_string()).collect();
            let key = crate::ltm::canonical_rotation(&path_strings);
            if seen_sets.insert(key) {
                all_paths.push(path);
            }
        }
    }
    all_paths
}

/// The optimized discovery loop in isolation (the integer-indexed path
/// inside `discover_loops_with_graph`), returning the same `all_paths`.
fn indexed_all_paths(
    results: &Results,
    link_offsets: &[LinkOffset],
    stocks: &[Ident<Canonical>],
) -> Vec<Vec<Ident<Canonical>>> {
    let mut all_paths: Vec<Vec<Ident<Canonical>>> = Vec::new();
    let mut seen_sets: HashSet<Vec<u32>> = HashSet::new();
    let search = IndexedSearch::build(link_offsets, stocks);
    let mut scratch = DfsScratch::new(&search);
    for step in 1..results.step_count {
        search.load_step_scores(results, step, &mut scratch);
        search.discover_step(&mut scratch, &mut seen_sets, &mut all_paths);
    }
    all_paths
}

fn paths_as_strings(paths: &[Vec<Ident<Canonical>>]) -> Vec<Vec<String>> {
    paths
        .iter()
        .map(|p| p.iter().map(|id| id.as_str().to_string()).collect())
        .collect()
}

/// Build a multi-step `Results` whose per-edge scores follow a deterministic
/// pseudo-random sequence, so the per-timestep edge sort order (and thus the
/// DFS traversal/pruning) varies across steps -- exercising the tie-breaking
/// and score-dependent branches in both implementations.
fn synthetic_results(n_offsets: usize, step_count: usize, seed: u64) -> Results {
    let step_size = n_offsets;
    let mut data = vec![0.0f64; step_size * step_count];
    // Step 0 is all NaN (PREVIOUS values don't exist), matching production;
    // discovery skips it. Remaining steps get varied finite scores, with a
    // few deliberate zeros/NaNs to exercise those branches.
    let mut state = seed | 1;
    let mut next = || {
        // xorshift64* -- deterministic, no external deps.
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545F4914F6CDD1D)
    };
    for slot in data.iter_mut().take(n_offsets) {
        *slot = f64::NAN;
    }
    for step in 1..step_count {
        for off in 0..n_offsets {
            let r = next();
            let v = match r % 16 {
                0 => 0.0,
                1 => f64::NAN,
                _ => {
                    let mag = ((r >> 8) % 1000) as f64 / 100.0;
                    if r & 1 == 0 { mag } else { -mag }
                }
            };
            data[step * step_size + off] = v;
        }
    }
    Results {
        offsets: HashMap::new(),
        data: data.into_boxed_slice(),
        step_size,
        step_count,
        specs: crate::results::Specs {
            start: 0.0,
            stop: (step_count - 1) as f64,
            dt: 1.0,
            save_step: 1.0,
            method: crate::results::Method::Euler,
            n_chunks: step_count,
        },
        is_vensim: false,
    }
}

#[test]
fn indexed_search_matches_reference_on_synthetic_graphs() {
    // A fully-connected-ish 5-node graph plus a couple of disconnected
    // nodes, several stocks, parallel edges, and a self-loop -- the shapes
    // the unit tests above exercise individually, combined and stressed
    // over many timesteps with varying scores.
    let names = ["a", "b", "c", "d", "e", "f", "g"];
    let mut edge_pairs: Vec<(&str, &str)> = Vec::new();
    for &from in &names[..5] {
        for &to in &names[..5] {
            edge_pairs.push((from, to)); // includes self-loops
        }
    }
    // A node ("g") that is only ever an edge target (no outbound edges)
    // and a duplicate (parallel) edge to stress tie-breaking / dedup.
    edge_pairs.push(("a", "g"));
    edge_pairs.push(("a", "b")); // parallel to the existing a->b

    let link_offsets: Vec<LinkOffset> = edge_pairs
        .iter()
        .enumerate()
        .map(|(i, (from, to))| ((Ident::new(from), Ident::new(to)), i))
        .collect();

    // Stocks include a node with no incident edges ("f") to mirror the
    // `test_stocks_without_outbound_edges` shape.
    let stocks: Vec<Ident<Canonical>> =
        ["a", "c", "e", "f"].iter().map(|s| Ident::new(s)).collect();

    // Run several independent seeds so the per-step sort order (and the
    // resulting traversal/pruning) varies widely.
    for seed in [1u64, 7, 42, 1000, 999_983] {
        let results = synthetic_results(link_offsets.len(), 40, seed);
        let reference = reference_all_paths(&results, &link_offsets, &stocks);
        let indexed = indexed_all_paths(&results, &link_offsets, &stocks);
        // Guard against a vacuous pass: a future fixture edit that produced
        // no loops would make the equality below trivially true.
        assert!(
            !reference.is_empty(),
            "synthetic fixture must produce loops (seed {seed})"
        );
        assert_eq!(
            paths_as_strings(&indexed),
            paths_as_strings(&reference),
            "IndexedSearch must discover the identical loop paths in the \
                 identical first-seen order as the SearchGraph reference \
                 (seed {seed})"
        );
    }
}

#[test]
fn indexed_search_matches_reference_element_level_names() {
    // Long element-level identifiers (the C-LEARN-style names whose string
    // hashing the optimization eliminates) over a denser graph.
    let names = [
        "population[nyc]",
        "births[nyc]",
        "deaths[nyc]",
        "population[boston]",
        "births[boston]",
        "migration_pressure[chicago]",
    ];
    let mut edge_pairs: Vec<(&str, &str)> = Vec::new();
    for &from in &names {
        for &to in &names {
            if from != to {
                edge_pairs.push((from, to));
            }
        }
    }
    let link_offsets: Vec<LinkOffset> = edge_pairs
        .iter()
        .enumerate()
        .map(|(i, (from, to))| ((Ident::new(from), Ident::new(to)), i))
        .collect();
    let stocks: Vec<Ident<Canonical>> = ["population[nyc]", "population[boston]"]
        .iter()
        .map(|s| Ident::new(s))
        .collect();

    for seed in [3u64, 17, 55, 12_345] {
        let results = synthetic_results(link_offsets.len(), 30, seed);
        let reference = reference_all_paths(&results, &link_offsets, &stocks);
        let indexed = indexed_all_paths(&results, &link_offsets, &stocks);
        assert!(
            !reference.is_empty(),
            "element-level fixture must produce loops (seed {seed})"
        );
        assert_eq!(
            paths_as_strings(&indexed),
            paths_as_strings(&reference),
            "element-level discovery must match the reference (seed {seed})"
        );
    }
}

// --- Discovery graph stats (GH #647 feasibility diagnostics) ---

#[test]
fn tarjan_scc_ids_identifies_cyclic_core() {
    // Graph: a -> b -> c -> a (3-cycle), c -> d (dead end), e isolated,
    // f -> g -> f (2-cycle).
    //   ids: a=0, b=1, c=2, d=3, e=4, f=5, g=6
    let adj: Vec<Vec<u32>> = vec![
        vec![1],    // a -> b
        vec![2],    // b -> c
        vec![0, 3], // c -> a, c -> d
        vec![],     // d
        vec![],     // e
        vec![6],    // f -> g
        vec![5],    // g -> f
    ];
    let (ids, sizes) = tarjan_scc_ids(&adj);
    assert_eq!(ids.len(), 7);
    // a, b, c share a component; f, g share a component; d and e are
    // singletons; no two of those groups share an id.
    assert_eq!(ids[0], ids[1]);
    assert_eq!(ids[1], ids[2]);
    assert_eq!(ids[5], ids[6]);
    assert_ne!(ids[0], ids[5]);
    assert_ne!(ids[0], ids[3]);
    assert_ne!(ids[0], ids[4]);
    assert_ne!(ids[3], ids[4]);
    // Component sizes: one 3, one 2, two 1s.
    let mut multi: Vec<u32> = sizes.iter().copied().filter(|&s| s > 1).collect();
    multi.sort_unstable();
    assert_eq!(multi, vec![2, 3]);
    assert_eq!(sizes[ids[0] as usize], 3);
    assert_eq!(sizes[ids[5] as usize], 2);
    assert_eq!(sizes[ids[3] as usize], 1);
}

#[test]
fn tarjan_scc_ids_handles_empty_and_self_loop() {
    let (ids, sizes) = tarjan_scc_ids(&[]);
    assert!(ids.is_empty());
    assert!(sizes.is_empty());

    // A self-loop is a size-1 SCC (callers detect self-edges separately).
    let adj: Vec<Vec<u32>> = vec![vec![0]];
    let (ids, sizes) = tarjan_scc_ids(&adj);
    assert_eq!(ids.len(), 1);
    assert_eq!(sizes[ids[0] as usize], 1);
}

#[test]
fn discovery_graph_stats_reports_structure_and_scores() {
    // Two link-score columns forming a 2-cycle (a <-> b), one dead-end
    // column (b -> c), and a stray non-link column. Scores at step 1:
    // a->b = 1.0 (unit), b->a = 0.5 (sub-unit), b->c = 0.0 (zero).
    // Scores at step 2: a->b = 3.0 (super-unit), b->a = 0.0 (zero,
    // breaking the cycle), b->c = 1.0.
    let mut offsets = HashMap::new();
    offsets.insert(Ident::new("$⁚ltm⁚link_score⁚a→b"), 0usize);
    offsets.insert(Ident::new("$⁚ltm⁚link_score⁚b→a"), 1usize);
    offsets.insert(Ident::new("$⁚ltm⁚link_score⁚b→c"), 2usize);
    offsets.insert(Ident::new("a"), 3usize);

    let data = vec![
        // step 0 (skipped by discovery; NaNs)
        f64::NAN,
        f64::NAN,
        f64::NAN,
        0.0,
        // step 1
        1.0,
        0.5,
        0.0,
        0.0,
        // step 2
        3.0,
        0.0,
        1.0,
        0.0,
    ];
    let results = Results {
        offsets,
        data: data.into_boxed_slice(),
        step_size: 4,
        step_count: 3,
        specs: crate::results::Specs {
            start: 0.0,
            stop: 2.0,
            dt: 1.0,
            save_step: 1.0,
            method: crate::results::Method::Euler,
            n_chunks: 3,
        },
        is_vensim: false,
    };

    let stocks = stock_list(&["a"]);
    let stats = discovery_graph_stats(&results, &stocks, &[], &[], &[1, 2]);

    assert_eq!(stats.n_edges, 3);
    // Nodes: a, b, c.
    assert_eq!(stats.n_nodes, 3);
    assert_eq!(stats.n_stocks, 1);
    // Static topology has one multi-node SCC: {a, b}.
    assert_eq!(stats.topology_scc_sizes, vec![2]);
    assert_eq!(stats.stocks_in_cyclic_core, 1);

    assert_eq!(stats.step_stats.len(), 2);
    let s1 = &stats.step_stats[0];
    assert_eq!(s1.step, 1);
    assert_eq!(s1.zero_edges, 1);
    assert_eq!(s1.unit_edges, 1);
    assert_eq!(s1.sub_unit_edges, 1);
    assert_eq!(s1.super_unit_edges, 0);
    assert_eq!(s1.max_abs_score, 1.0);
    // With the zero edge dropped, the a <-> b cycle survives at step 1.
    assert_eq!(s1.nonzero_scc_sizes, vec![2]);
    assert_eq!(s1.stocks_in_nonzero_core, 1);

    let s2 = &stats.step_stats[1];
    assert_eq!(s2.step, 2);
    assert_eq!(s2.zero_edges, 1);
    assert_eq!(s2.unit_edges, 1);
    assert_eq!(s2.sub_unit_edges, 0);
    assert_eq!(s2.super_unit_edges, 1);
    assert_eq!(s2.max_abs_score, 3.0);
    // b -> a is zero at step 2, so no multi-node nonzero SCC remains.
    assert!(s2.nonzero_scc_sizes.is_empty());
    assert_eq!(s2.stocks_in_nonzero_core, 0);
}

// --- Mid-step deadline enforcement (the in-DFS budget check) ---

/// Build an IndexedSearch over `a -> b -> c -> a` (single stock `a`) with
/// every per-step edge score populated as 1.0, plus a scratch ready for
/// `discover_step`. Bypasses `load_step_scores` so no `Results` is needed.
fn cycle_search_and_scratch() -> (IndexedSearch, DfsScratch) {
    let link_offsets: Vec<LinkOffset> = vec![
        ((Ident::new("a"), Ident::new("b")), 0),
        ((Ident::new("b"), Ident::new("c")), 1),
        ((Ident::new("c"), Ident::new("a")), 2),
    ];
    let stocks = stock_list(&["a"]);
    let search = IndexedSearch::build(&link_offsets, &stocks);
    let mut scratch = DfsScratch::new(&search);
    for (node, edges) in search.adj.iter().enumerate() {
        scratch.step_adj[node] = edges
            .iter()
            .map(|e| StepEdge {
                to: e.to,
                score: 1.0,
            })
            .collect();
    }
    (search, scratch)
}

#[test]
fn dfs_deadline_expires_mid_step() {
    // On dense element-level graphs a SINGLE timestep's DFS can run for
    // hours (GH #647), so the budget must be enforced inside the DFS, not
    // only between timesteps. With an already-passed deadline and the
    // visit counter seeded so the very first visit performs the
    // (interval-amortized) clock check, the DFS must flag expiry and bail
    // without recording the cycle. The counter seeding stands in for the
    // thousands of visits a large graph would need to reach the check
    // naturally -- per the test-budget policy, tests must not build
    // fixtures big enough to trip production thresholds for real.
    let (search, mut scratch) = cycle_search_and_scratch();
    scratch.deadline = Some(Instant::now());
    scratch.visit_count = DEADLINE_CHECK_INTERVAL - 1;

    let mut seen: HashSet<Vec<u32>> = HashSet::new();
    let mut paths: Vec<Vec<Ident<Canonical>>> = Vec::new();
    search.discover_step(&mut scratch, &mut seen, &mut paths);

    assert!(
        scratch.deadline_expired,
        "an expired deadline must be detected inside the step's DFS"
    );
    assert!(
        paths.is_empty(),
        "the DFS must unwind without recording loops once the deadline expired"
    );
}

#[test]
fn dfs_unexpired_deadline_still_finds_loops() {
    // The deadline machinery must not suppress discovery: with no deadline
    // the cycle is found, and with a far-future deadline (clock check
    // exercised via the seeded counter) it is found too.
    let (search, mut scratch) = cycle_search_and_scratch();
    let mut seen: HashSet<Vec<u32>> = HashSet::new();
    let mut paths: Vec<Vec<Ident<Canonical>>> = Vec::new();
    search.discover_step(&mut scratch, &mut seen, &mut paths);
    assert!(!scratch.deadline_expired);
    assert_eq!(
        paths_as_strings(&paths),
        vec![vec!["a".to_string(), "b".to_string(), "c".to_string()]],
        "the a -> b -> c cycle must be discovered on an unbudgeted run"
    );

    let (search2, mut scratch2) = cycle_search_and_scratch();
    scratch2.deadline = Some(Instant::now() + Duration::from_secs(3600));
    scratch2.visit_count = DEADLINE_CHECK_INTERVAL - 1;
    let mut seen2: HashSet<Vec<u32>> = HashSet::new();
    let mut paths2: Vec<Vec<Ident<Canonical>>> = Vec::new();
    search2.discover_step(&mut scratch2, &mut seen2, &mut paths2);
    assert!(
        !scratch2.deadline_expired,
        "a far-future deadline must not be reported as expired"
    );
    assert_eq!(
        paths_as_strings(&paths2),
        vec![vec!["a".to_string(), "b".to_string(), "c".to_string()]],
        "the cycle must still be discovered when the deadline check fires but has not passed"
    );
}

/// Compile an arrayed reducer-in-feedback model with LTM discovery enabled,
/// simulate it, and run the full discovery pipeline. Uses the bare
/// `causal_graph_from_element_edges` constructor (no module sub-graphs);
/// production `analysis::analyze_model` uses the `_with_modules` enriching
/// variant instead, but module enrichment is orthogonal to cross-agg
/// recovery, and the production wiring is covered end-to-end by
/// `discovery_recovers_cross_agg_loops_matches_exhaustive` in
/// tests/simulate_ltm.rs. Returns the `DiscoveryResult`.
///
/// `growth[r] = SUM(pop[*]) * 0.05` over `elems`: one scalar synthetic agg,
/// one petal per element, so the cross-agg recovery (GH #696) is exercised.
fn discover_reducer_feedback(elems: &[&str]) -> DiscoveryResult {
    use crate::datamodel::{self, Equation, Variable};
    use salsa::Setter;

    let project = datamodel::Project {
        name: "reducer_feedback".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 5.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension::named(
            "Region".to_string(),
            elems.iter().map(|s| s.to_string()).collect(),
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Stock(datamodel::Stock {
                    ident: "pop".to_string(),
                    equation: Equation::Arrayed(
                        vec!["Region".to_string()],
                        elems
                            .iter()
                            .enumerate()
                            .map(|(i, e)| {
                                let init = (1000.0 / 3f64.powi(i as i32)).round();
                                (e.to_string(), format!("{init}"), None, None)
                            })
                            .collect(),
                        None,
                        false,
                    ),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["growth".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(datamodel::Flow {
                    ident: "growth".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        "SUM(pop[*]) * 0.05".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let mut db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel_incremental(&mut db, &project, None);
    let sp = sync.project;
    sp.set_ltm_enabled(&mut db).to(true);
    sp.set_ltm_discovery_mode(&mut db).to(true);
    let source_model = *sp.models(&db).get("main").unwrap();

    let compiled = crate::db::compile_project_incremental(&db, sp, "main").unwrap();
    let mut vm = crate::vm::Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let element_edges = crate::db::model_element_causal_edges(&db, source_model, sp);
    let causal_graph = crate::db::causal_graph_from_element_edges(element_edges);
    let stocks: Vec<Ident<Canonical>> = element_edges
        .stocks
        .iter()
        .map(|s| Ident::new(s.as_str()))
        .collect();
    let ltm = crate::db::model_ltm_variables(&db, source_model, sp);
    let dm_dims = crate::db::project_datamodel_dims(&db, sp);

    // These fixtures contain no modules, so the per-exit-port recompute
    // never fires; an empty output-port map is correct.
    discover_loops_with_graph(
        &results,
        &causal_graph,
        &stocks,
        &ltm.vars,
        dm_dims,
        &SubModelOutputPorts::new(),
        None,
    )
    .unwrap()
}

/// GH #696: discovery stitches the per-element petals into cross-element
/// loops. On the 3-element reducer-in-feedback model it recovers all 7
/// (3 single-petal + 3 pair + 1 triple), and the flag is not raised when
/// well under budget.
#[test]
fn discovery_recovers_cross_agg_loops_end_to_end() {
    let result = discover_reducer_feedback(&["a", "b", "c"]);
    assert_eq!(
        result.loops.len(),
        7,
        "discovery must recover 3 single + 3 pair + 1 triple loops; got {:?}",
        result
            .loops
            .iter()
            .map(|l| l
                .loop_info
                .links
                .iter()
                .map(|k| k.from.as_str())
                .collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
    assert!(
        !result.agg_recovery_truncated,
        "a 3-petal model is well under the production budget"
    );
    // Loops of three distinct sizes appear (single-petal, pair, triple).
    let sizes: HashSet<usize> = result
        .loops
        .iter()
        .map(|l| l.loop_info.links.len())
        .collect();
    assert!(
        sizes.contains(&2) && sizes.contains(&4) && sizes.contains(&6),
        "expected loop link-counts 2/4/6; got {sizes:?}"
    );
}

/// The cross-agg loop-count budget clips discovery's recovery and raises
/// `agg_recovery_truncated`, using a test-only override so a tiny fixture
/// trips it (per docs/dev/rust.md#test-time-budgets).
#[test]
fn discovery_cross_agg_recovery_respects_budget() {
    // Budget of 1 lets at most one stitched cross-agg loop through; the
    // 3 single-petal elementary loops are emitted by the DFS regardless
    // (they are not stitched), so we expect 3 petals + 1 stitched = 4.
    let _guard = crate::db::AggLoopBudgetGuard::new(1);
    let result = discover_reducer_feedback(&["a", "b", "c"]);
    assert!(
        result.agg_recovery_truncated,
        "a budget of 1 must clip the 4 stitched loops and flag truncation"
    );
    // The single petals always survive (3); only one stitched loop fit.
    assert_eq!(
        result.loops.len(),
        4,
        "3 elementary petals + 1 budgeted stitched loop; got {}",
        result.loops.len()
    );
}

/// A model with no hoisted reducer (plain logistic-style feedback) is
/// unaffected by the petal stitcher: no agg nodes means no petals, so the
/// discovered loop set and the truncation flag are exactly as before.
#[test]
fn discovery_no_agg_model_unaffected_by_stitching() {
    use crate::datamodel::{self, Equation, Variable};
    use salsa::Setter;

    let project = datamodel::Project {
        name: "no_agg".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 5.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Stock(datamodel::Stock {
                    ident: "population".to_string(),
                    equation: Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["births".to_string()],
                    outflows: vec!["deaths".to_string()],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(datamodel::Flow {
                    ident: "births".to_string(),
                    equation: Equation::Scalar("population * 0.1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(datamodel::Flow {
                    ident: "deaths".to_string(),
                    equation: Equation::Scalar("population * population * 0.0001".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let mut db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel_incremental(&mut db, &project, None);
    let sp = sync.project;
    sp.set_ltm_enabled(&mut db).to(true);
    sp.set_ltm_discovery_mode(&mut db).to(true);
    let source_model = *sp.models(&db).get("main").unwrap();
    let compiled = crate::db::compile_project_incremental(&db, sp, "main").unwrap();
    let mut vm = crate::vm::Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();
    let element_edges = crate::db::model_element_causal_edges(&db, source_model, sp);
    let causal_graph = crate::db::causal_graph_from_element_edges(element_edges);
    let stocks: Vec<Ident<Canonical>> = element_edges
        .stocks
        .iter()
        .map(|s| Ident::new(s.as_str()))
        .collect();
    let ltm = crate::db::model_ltm_variables(&db, source_model, sp);
    let dm_dims = crate::db::project_datamodel_dims(&db, sp);
    // These fixtures contain no modules; an empty output-port map is correct.
    let result = discover_loops_with_graph(
        &results,
        &causal_graph,
        &stocks,
        &ltm.vars,
        dm_dims,
        &SubModelOutputPorts::new(),
        None,
    )
    .unwrap();

    assert!(
        !result.agg_recovery_truncated,
        "a no-agg model must never flag agg-recovery truncation"
    );
    // Two feedback loops (reinforcing births, balancing deaths), no agg.
    assert_eq!(
        result.loops.len(),
        2,
        "the no-agg model has exactly two loops"
    );
    for l in &result.loops {
        assert!(
            l.loop_info
                .links
                .iter()
                .all(|k| !k.from.as_str().contains("\u{205A}agg\u{205A}")),
            "no synthetic agg can appear in a no-agg model's loops"
        );
    }
}

/// GH #698 / PR #705 r3353758167: `recompute_module_input_edge_series` must
/// strip element subscripts before its name-sensitive lookups. Discovery
/// runs on the ELEMENT-LEVEL graph, so an arrayed loop edge carries
/// subscripts (`s[nyc] -> m -> growth[nyc]`); the bare `ModuleInput.src`
/// (`s`) and bare-keyed `variables()` map (`growth`) only match after
/// stripping `[nyc]`. The exhaustive twin already strips at
/// db/ltm/mod.rs:438-485, so this restores parity.
///
/// Genuine red-green of exactly the matching code: a real compiled
/// multi-output module project supplies the element graph, the module
/// sub-graph + variable map, the `m·$⁚ltm⁚path⁚input_val⁚{idx}` pathway
/// series, and the emission-derived port map; we hand a `links` chain whose
/// non-module nodes carry `[nyc]` subscripts (as the element graph would
/// for an arrayed loop). Before the fix the exact `== link.from` /
/// `variables().get(y)` matches fail on the subscripted names and the
/// function returns `None`; after the fix they resolve and it returns
/// `Some` (the `pos`-pathway series the loop traverses, +1), distinct from
/// the wrong-signed `neg` composite the `None` fallback would keep.
#[test]
fn recompute_strips_element_subscripts_before_port_match() {
    use crate::datamodel::{self, Equation};
    use crate::ltm::LinkPolarity;
    use salsa::Setter;

    // Sub-model exposing two opposite-signed outputs from one input port.
    let passthrough = datamodel::Model {
        name: "passthrough".to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "input_val".to_string(),
                equation: Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat {
                    can_be_module_input: true,
                    ..datamodel::Compat::default()
                },
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "pos".to_string(),
                equation: Equation::Scalar("input_val * 0.02".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "neg".to_string(),
                equation: Equation::Scalar("0 - input_val".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }),
        ],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    };
    let main = datamodel::Model {
        name: "main".to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "s".to_string(),
                equation: Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["growth".to_string()],
                outflows: vec![],
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }),
            datamodel::Variable::Module(datamodel::Module {
                ident: "m".to_string(),
                model_name: "passthrough".to_string(),
                documentation: String::new(),
                units: None,
                references: vec![datamodel::ModuleReference {
                    src: "s".to_string(),
                    dst: "m.input_val".to_string(),
                }],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "growth".to_string(),
                equation: Equation::Scalar("m.pos * 0.1".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "watcher".to_string(),
                equation: Equation::Scalar("m.neg".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }),
        ],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    };
    let project = datamodel::Project {
        name: "subscript_match".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 4.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![main, passthrough],
        source: None,
        ai_information: None,
    };

    let mut db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel_incremental(&mut db, &project, None);
    let sp = sync.project;
    sp.set_ltm_enabled(&mut db).to(true);
    sp.set_ltm_discovery_mode(&mut db).to(true);
    let source_model = *sp.models(&db).get("main").unwrap();

    let compiled = crate::db::compile_project_incremental(&db, sp, "main").unwrap();
    let mut vm = crate::vm::Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let element_edges = crate::db::model_element_causal_edges(&db, source_model, sp);
    let causal_graph = crate::db::causal_graph_from_element_edges_with_modules(
        &db,
        source_model,
        sp,
        element_edges,
    );
    let sub_model_ports = crate::analysis::build_sub_model_output_ports(&db, sp);

    // Hand-built ELEMENT-LEVEL loop chain: the same `s -> m -> growth -> s`
    // cycle the scalar graph forms, but with `[nyc]` subscripts on the
    // non-module nodes as the element graph stamps them for an arrayed
    // loop. `m` (a module instance) stays unsubscripted, matching the
    // element graph.
    let link = |from: &str, to: &str| Link {
        from: Ident::new(from),
        to: Ident::new(to),
        polarity: LinkPolarity::Unknown,
    };
    let links = vec![
        link("s[nyc]", "m"),
        link("m", "growth[nyc]"),
        link("growth[nyc]", "s[nyc]"),
    ];

    let series = recompute_module_input_edge_series(
        &causal_graph,
        &results,
        &links,
        0, // the `s[nyc] -> m` edge
        results.step_count,
        &sub_model_ports,
    );

    let series = series.expect(
        "recompute must resolve the entry/exit ports after stripping the `[nyc]` subscripts \
             from `s[nyc]` (entry match vs bare ModuleInput.src `s`) and `growth[nyc]` (exit \
             reader lookup in the bare-keyed variables map); before the fix the exact match \
             returns None and the wrong-signed neg composite stands. PR #705 r3353758167.",
    );
    // The loop reads m·pos (positive gain); the recomputed series follows
    // that pathway (+1 at every settled step), never the neg port (-1).
    let settled = *series
        .iter()
        .rev()
        .find(|v| v.is_finite() && **v != 0.0)
        .expect("recomputed series must have a finite non-zero settled value");
    assert!(
        settled > 0.0,
        "recomputed series follows the m·pos pathway (+); got {settled}. PR #705 r3353758167."
    );
}
