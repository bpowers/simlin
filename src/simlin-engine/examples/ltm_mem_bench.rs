// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Memory and timing benchmark for LTM circuit enumeration.
//!
//! Two modes:
//!
//! 1. Real model (default): parse a Vensim MDL file, build the causal
//!    graph, and enumerate elementary circuits.  Reports nodes, edges,
//!    stocks, wall-clock time, circuit count, and peak RSS.  Used to
//!    track regressions on dense single-SCC graphs like wrld3.
//!
//! 2. Synthetic graph (`--synthetic` first arg): build a graph with
//!    `num_sccs` strongly-connected components of mixed sizes embedded
//!    in a `padding_nodes`-sized acyclic feeder/sink padding, so the
//!    total node count vastly exceeds any individual SCC size.  Drives
//!    the per-SCC allocation regression in issue #460: the pre-fix
//!    `JohnsonState` allocates O(|graph|) per SCC even when only
//!    O(|SCC|) is used.
//!
//! Usage:
//!   cargo run --release --example ltm_mem_bench -- [mdl] [budget]
//!   cargo run --release --example ltm_mem_bench -- --synthetic
//!
//! Examples:
//!   cargo run --release --example ltm_mem_bench -- \
//!       test/metasd/WRLD3-03/wrld3-03.mdl 10000000
//!   cargo run --release --example ltm_mem_bench -- --synthetic
//!
//! Designed as a repeatable measurement harness for iterating on the
//! LTM enumeration algorithm without having to reason about the full
//! salsa pipeline.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use simlin_engine::common::{Canonical, Ident};
use simlin_engine::db::{
    CausalEdgesResult, SimlinDb, causal_graph_from_edges, model_causal_edges, sync_from_datamodel,
};
use simlin_engine::ltm::CausalGraph;
use simlin_engine::open_vensim;

/// Tracking allocator: counts the number of `alloc` calls and the total
/// bytes requested.  The benchmark zeros the counters before each
/// enumeration window and reports the deltas to surface the per-SCC
/// transient allocation churn that issue #460 targets.  Peak RSS
/// (`VmPeak` / `VmHWM`) is page-granular and doesn't move when the
/// allocator reuses freed memory between SCCs, so byte counts are the
/// load-bearing metric here.
struct CountingAlloc;

static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn alloc_snapshot() -> (u64, usize) {
    (
        ALLOC_COUNT.load(Ordering::Relaxed),
        ALLOC_BYTES.load(Ordering::Relaxed),
    )
}

fn read_proc_status_kb(key: &str) -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            // lines look like: "VmPeak:\t  12345 kB"
            let rest = rest.trim_start_matches(':').trim();
            let value = rest.trim_end_matches(" kB").trim();
            return value.parse().ok();
        }
    }
    None
}

fn print_mem(label: &str) {
    let peak = read_proc_status_kb("VmPeak").unwrap_or(0);
    let hwm = read_proc_status_kb("VmHWM").unwrap_or(0);
    let rss = read_proc_status_kb("VmRSS").unwrap_or(0);
    eprintln!(
        "[mem {label:>8}] VmPeak={:>9.3} MiB  VmHWM={:>9.3} MiB  VmRSS={:>9.3} MiB",
        peak as f64 / 1024.0,
        hwm as f64 / 1024.0,
        rss as f64 / 1024.0,
    );
}

fn scc_stats(
    edges: &HashMap<Ident<Canonical>, Vec<Ident<Canonical>>>,
) -> (usize, Vec<usize>, Vec<Vec<Ident<Canonical>>>) {
    // Tarjan's SCC iteratively, so we don't blow the stack on big graphs.
    // Returns (num_sccs, sorted_desc_sizes, non_trivial_sccs).
    let mut all_nodes: std::collections::HashSet<&Ident<Canonical>> =
        std::collections::HashSet::new();
    for (from, tos) in edges {
        all_nodes.insert(from);
        for to in tos {
            all_nodes.insert(to);
        }
    }
    let mut nodes: Vec<&Ident<Canonical>> = all_nodes.into_iter().collect();
    nodes.sort_by_key(|n| n.as_str());
    let index_of: HashMap<&Ident<Canonical>, usize> =
        nodes.iter().enumerate().map(|(i, n)| (*n, i)).collect();

    // Build per-index successor lists
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (from, tos) in edges {
        if let Some(&fi) = index_of.get(from) {
            for to in tos {
                if let Some(&ti) = index_of.get(to) {
                    succ[fi].push(ti);
                }
            }
        }
    }

    let mut index = 0usize;
    let mut indices: Vec<i64> = vec![-1; nodes.len()];
    let mut lowlinks: Vec<i64> = vec![-1; nodes.len()];
    let mut on_stack: Vec<bool> = vec![false; nodes.len()];
    let mut stack: Vec<usize> = Vec::new();
    let mut sccs: Vec<Vec<usize>> = Vec::new();

    enum Frame {
        Enter(usize),
        Resume { v: usize, next_child: usize },
    }

    for start in 0..nodes.len() {
        if indices[start] != -1 {
            continue;
        }
        let mut frames: Vec<Frame> = vec![Frame::Enter(start)];
        while let Some(frame) = frames.pop() {
            match frame {
                Frame::Enter(v) => {
                    indices[v] = index as i64;
                    lowlinks[v] = index as i64;
                    index += 1;
                    stack.push(v);
                    on_stack[v] = true;
                    frames.push(Frame::Resume { v, next_child: 0 });
                }
                Frame::Resume { v, next_child } => {
                    if next_child < succ[v].len() {
                        let w = succ[v][next_child];
                        frames.push(Frame::Resume {
                            v,
                            next_child: next_child + 1,
                        });
                        if indices[w] == -1 {
                            frames.push(Frame::Enter(w));
                        } else if on_stack[w] && indices[w] < lowlinks[v] {
                            lowlinks[v] = indices[w];
                        }
                    } else {
                        // child iteration done; propagate lowlink to parent
                        if let Some(Frame::Resume {
                            v: parent,
                            next_child: _,
                        }) = frames.last_mut()
                            && lowlinks[v] < lowlinks[*parent]
                        {
                            lowlinks[*parent] = lowlinks[v];
                        }
                        if lowlinks[v] == indices[v] {
                            let mut scc = Vec::new();
                            loop {
                                let w = stack.pop().unwrap();
                                on_stack[w] = false;
                                scc.push(w);
                                if w == v {
                                    break;
                                }
                            }
                            sccs.push(scc);
                        }
                    }
                }
            }
        }
    }

    let mut sizes: Vec<usize> = sccs.iter().map(|s| s.len()).collect();
    sizes.sort_unstable_by(|a, b| b.cmp(a));

    // Non-trivial SCCs = size > 1 OR (size == 1 AND has self-loop)
    let mut non_trivial: Vec<Vec<Ident<Canonical>>> = Vec::new();
    for scc in &sccs {
        if scc.len() > 1 {
            non_trivial.push(scc.iter().map(|&i| nodes[i].clone()).collect());
        } else {
            let v = scc[0];
            if succ[v].contains(&v) {
                non_trivial.push(vec![nodes[v].clone()]);
            }
        }
    }

    (sccs.len(), sizes, non_trivial)
}

fn connected_component_stats(
    edges: &HashMap<Ident<Canonical>, Vec<Ident<Canonical>>>,
) -> (usize, Vec<usize>) {
    // Undirected connected components: for quick structural sanity check.
    let mut undir: HashMap<&Ident<Canonical>, Vec<&Ident<Canonical>>> = HashMap::new();
    for (from, tos) in edges {
        undir.entry(from).or_default();
        for to in tos {
            undir.entry(from).or_default().push(to);
            undir.entry(to).or_default().push(from);
        }
    }
    let all_nodes: std::collections::HashSet<&Ident<Canonical>> = undir.keys().copied().collect();
    let mut seen: std::collections::HashSet<&Ident<Canonical>> = std::collections::HashSet::new();
    let mut sizes = Vec::new();
    for start in &all_nodes {
        if seen.contains(start) {
            continue;
        }
        let mut stack = vec![*start];
        let mut size = 0;
        while let Some(node) = stack.pop() {
            if !seen.insert(node) {
                continue;
            }
            size += 1;
            if let Some(neighbors) = undir.get(node) {
                for n in neighbors {
                    if !seen.contains(n) {
                        stack.push(*n);
                    }
                }
            }
        }
        sizes.push(size);
    }
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    (all_nodes.len(), sizes)
}

/// Build a synthetic graph with multiple SCCs of mixed sizes embedded
/// in a much larger acyclic node space.
///
/// Each SCC is a complete directed cycle (n nodes, n edges in a ring),
/// which keeps the per-SCC enumeration cost trivial (1 cycle per SCC)
/// and the total budget bounded -- the benchmark measures the
/// transient JohnsonState allocation, not enumeration runtime.
///
/// Layout:
///   * `padding_acyclic_nodes` feeder/sink nodes whose names sort
///     before/after any SCC member.  Each padding node contributes one
///     directed edge to a sink, so the resulting graph has many singleton
///     SCCs that get skipped fast by the trivial-SCC filter, plus the
///     non-trivial cycle SCCs we constructed.
///   * `scc_sizes` describes the cycle SCCs to construct: each entry is
///     a (count, nodes_per_scc) pair.  E.g., `[(50, 5), (50, 10), (80,
///     50), (20, 100)]` builds 50 5-cycles, 50 10-cycles, 80 50-cycles,
///     and 20 100-cycles -- the same shape the issue calls out as the
///     pathological case for the pre-fix per-SCC allocation pattern.
fn build_synthetic_edges_result(
    scc_sizes: &[(usize, usize)],
    padding_acyclic_nodes: usize,
) -> CausalEdgesResult {
    let mut edges: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut next_scc_idx = 0usize;
    for &(count, size) in scc_sizes {
        for _ in 0..count {
            let scc_idx = next_scc_idx;
            next_scc_idx += 1;
            // Names: scc_{idx:06}_n{member:04} -- six-digit zero padded
            // SCC index keeps lex order stable across runs and across
            // SCC sizes.
            let names: Vec<String> = (0..size)
                .map(|i| format!("scc_{scc_idx:06}_n{i:04}"))
                .collect();
            for i in 0..size {
                let from = names[i].clone();
                let to = names[(i + 1) % size].clone();
                edges.entry(from).or_default().insert(to);
            }
        }
    }
    // Padding feeders: feed_NNN -> feed_NNN_sink (a fresh sink per feeder
    // so each is its own singleton SCC and the padding does not
    // accidentally form cycles).  These exist to inflate the global
    // node count without contributing cycles.
    for i in 0..padding_acyclic_nodes {
        let from = format!("feed_{i:06}");
        let to = format!("sink_{i:06}");
        edges.entry(from).or_default().insert(to);
    }
    CausalEdgesResult {
        edges,
        stocks: BTreeSet::new(),
        dynamic_modules: HashMap::new(),
    }
}

fn run_enumeration(label: &str, edges_result: &CausalEdgesResult, budget: usize) {
    let graph: CausalGraph = causal_graph_from_edges(edges_result);

    let n_nodes_with_outedges = graph.edges().len();
    let total_outedges: usize = graph.edges().values().map(|v| v.len()).sum();
    let (n_total_nodes, cc_sizes) = connected_component_stats(graph.edges());
    eprintln!(
        "[{label}] graph: nodes-with-outedges={n_nodes_with_outedges}  \
         all-nodes(by edges)={n_total_nodes}  total-outedges={total_outedges}  stocks={}",
        graph.stocks().len()
    );
    eprintln!(
        "[{label}] weakly-connected components: {} (largest = {})",
        cc_sizes.len(),
        cc_sizes.first().copied().unwrap_or(0)
    );

    let (n_sccs, scc_sizes, non_trivial) = scc_stats(graph.edges());
    eprintln!(
        "[{label}] strongly-connected components: {n_sccs} (largest = {})",
        scc_sizes.first().copied().unwrap_or(0)
    );
    eprintln!(
        "[{label}] scc sizes (top 15): {:?}",
        &scc_sizes[..scc_sizes.len().min(15)]
    );
    let loop_carrying: usize = non_trivial.iter().map(|s| s.len()).sum();
    eprintln!(
        "[{label}] non-trivial SCCs (size>1 or self-loop): {}  loop-carrying nodes: {loop_carrying}",
        non_trivial.len()
    );
    print_mem(label);

    eprintln!("[{label}] enumerating circuits...");
    let (alloc_count_before, alloc_bytes_before) = alloc_snapshot();
    let t0 = Instant::now();
    let result = graph.find_indexed_circuits_with_limit(budget);
    let elapsed = t0.elapsed();
    let (alloc_count_after, alloc_bytes_after) = alloc_snapshot();
    let alloc_count_delta = alloc_count_after - alloc_count_before;
    let alloc_bytes_delta = alloc_bytes_after - alloc_bytes_before;

    match result {
        Ok((names, circuits)) => {
            eprintln!(
                "[{label}] circuits (deduplicated): {}  unique node names: {}",
                circuits.len(),
                names.len(),
            );
            let lens: Vec<usize> = circuits.iter().map(|c| c.len()).collect();
            if !lens.is_empty() {
                let min = *lens.iter().min().unwrap();
                let max = *lens.iter().max().unwrap();
                let sum: usize = lens.iter().sum();
                eprintln!(
                    "[{label}] circuit lengths: min={min}  max={max}  mean={:.1}",
                    sum as f64 / lens.len() as f64
                );
            }
        }
        Err(_) => {
            eprintln!("[{label}] TRUNCATED at budget {budget}");
        }
    }
    eprintln!("[{label}] elapsed: {elapsed:?}");
    eprintln!(
        "[{label}] enumeration allocations: count={alloc_count_delta}  bytes={alloc_bytes_delta} \
         ({:.3} MiB)",
        alloc_bytes_delta as f64 / (1024.0 * 1024.0)
    );
    print_mem(&format!("{label}-done"));
}

fn run_synthetic(budget: usize) {
    eprintln!("=== LTM synthetic-graph benchmark ===");
    eprintln!("budget: {budget}");
    print_mem("startup");

    // Issue #460's measurement target: 10,000-ish nodes spread across
    // 200 SCCs of mixed sizes (5/10/50/100).  In practice pure ring
    // SCCs of this size enumerate trivially (one circuit per SCC); the
    // benchmark is sensitive to the per-SCC JohnsonState allocation,
    // not enumeration runtime.  Padding inflates the global node count
    // so the pre-fix behaviour pays O(|graph|) per non-trivial SCC.
    let scc_sizes: &[(usize, usize)] = &[(50, 5), (50, 10), (80, 50), (20, 100)];
    // Total cycle nodes: 50*5 + 50*10 + 80*50 + 20*100 = 250+500+4000+2000 = 6750
    // Plus padding to keep the total around 10K.
    let padding_acyclic_nodes = 3500usize;

    let cycle_nodes: usize = scc_sizes.iter().map(|(c, s)| c * s).sum();
    let total_sccs: usize = scc_sizes.iter().map(|(c, _)| c).sum();
    eprintln!(
        "synthetic params: {} non-trivial SCCs ({} cycle nodes), {} padding nodes -> ~{} total",
        total_sccs,
        cycle_nodes,
        padding_acyclic_nodes,
        cycle_nodes + 2 * padding_acyclic_nodes
    );

    let edges_result = build_synthetic_edges_result(scc_sizes, padding_acyclic_nodes);
    print_mem("built");

    run_enumeration("synthetic", &edges_result, budget);
}

fn run_real_model(mdl_path: &str, budget: usize) {
    eprintln!("=== LTM memory/time benchmark ===");
    eprintln!("model:  {mdl_path}");
    eprintln!("budget: {budget}");
    print_mem("startup");

    let mdl = fs::read_to_string(mdl_path).expect("read model file");
    let datamodel = open_vensim(&mdl).expect("parse MDL");
    eprintln!(
        "parsed: {} models, root variables = {}",
        datamodel.models.len(),
        datamodel
            .models
            .first()
            .map(|m| m.variables.len())
            .unwrap_or(0)
    );
    print_mem("parsed");

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    // The root model is always the first entry of `datamodel.models`;
    // looking it up by that exact name avoids the trap of "any non-stdlib
    // model" which would match arbitrary submodels on projects that have
    // them (sync.models is a HashMap, so .iter().find() is not stable).
    let root_name = datamodel
        .models
        .first()
        .map(|m| m.name.as_str())
        .expect("datamodel must contain at least one model");
    let synced = sync
        .models
        .get(root_name)
        .expect("root model must be present in sync result");
    eprintln!("root model: '{root_name}'");
    print_mem("synced");

    let t0 = Instant::now();
    let edges_result = model_causal_edges(&db, synced.source, sync.project);
    let edges_dur = t0.elapsed();
    eprintln!(
        "model_causal_edges: {:?}  ({} source nodes, {} stocks)",
        edges_dur,
        edges_result.edges.len(),
        edges_result.stocks.len(),
    );

    let graph: CausalGraph = causal_graph_from_edges(edges_result);
    let n_nodes_with_outedges = graph.edges().len();
    let total_outedges: usize = graph.edges().values().map(|v| v.len()).sum();
    let (n_total_nodes, cc_sizes) = connected_component_stats(graph.edges());
    eprintln!(
        "graph:  nodes-with-outedges={n_nodes_with_outedges}  all-nodes(by edges)={n_total_nodes}  \
         total-outedges={total_outedges}  stocks={}",
        graph.stocks().len()
    );
    eprintln!(
        "weakly-connected components: {} (largest = {})",
        cc_sizes.len(),
        cc_sizes.first().copied().unwrap_or(0)
    );
    eprintln!("cc sizes: {:?}", &cc_sizes[..cc_sizes.len().min(10)]);

    let (n_sccs, scc_sizes, non_trivial) = scc_stats(graph.edges());
    eprintln!(
        "strongly-connected components: {n_sccs} (largest = {})",
        scc_sizes.first().copied().unwrap_or(0)
    );
    eprintln!("scc sizes: {:?}", &scc_sizes[..scc_sizes.len().min(15)]);
    let loop_carrying: usize = non_trivial.iter().map(|s| s.len()).sum();
    eprintln!(
        "non-trivial SCCs (size>1 or self-loop): {}  loop-carrying nodes: {loop_carrying}",
        non_trivial.len()
    );
    print_mem("graph");

    eprintln!("\n--- enumerating circuits ---");
    let (alloc_count_before, alloc_bytes_before) = alloc_snapshot();
    let t0 = Instant::now();
    // Use the indexed API (names table + u32 paths) so the benchmark
    // reflects the post-H13 memory profile -- the legacy
    // `Vec<Vec<Ident<Canonical>>>` path still exists for callers that
    // need owned idents, but production LTM flows through the indexed
    // view exclusively.
    let result = graph.find_indexed_circuits_with_limit(budget);
    let elapsed = t0.elapsed();
    let (alloc_count_after, alloc_bytes_after) = alloc_snapshot();
    let alloc_count_delta = alloc_count_after - alloc_count_before;
    let alloc_bytes_delta = alloc_bytes_after - alloc_bytes_before;

    match result {
        Ok((names, circuits)) => {
            eprintln!(
                "circuits (deduplicated): {}  unique node names: {}",
                circuits.len(),
                names.len(),
            );
            let lens: Vec<usize> = circuits.iter().map(|c| c.len()).collect();
            if !lens.is_empty() {
                let min = *lens.iter().min().unwrap();
                let max = *lens.iter().max().unwrap();
                let sum: usize = lens.iter().sum();
                eprintln!(
                    "circuit lengths: min={min}  max={max}  mean={:.1}",
                    sum as f64 / lens.len() as f64
                );
            }
        }
        Err(_) => {
            eprintln!("TRUNCATED at budget {budget} (caller should treat as 'too many')");
        }
    }
    eprintln!("elapsed: {elapsed:?}");
    eprintln!(
        "enumeration allocations: count={alloc_count_delta}  bytes={alloc_bytes_delta} \
         ({:.3} MiB)",
        alloc_bytes_delta as f64 / (1024.0 * 1024.0)
    );
    print_mem("done");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let first = args.get(1).cloned().unwrap_or_default();

    if first == "--synthetic" {
        let budget: usize = args
            .get(2)
            .and_then(|s| s.parse().ok())
            .unwrap_or(usize::MAX);
        run_synthetic(budget);
        return;
    }

    let mdl_path = if first.is_empty() {
        "test/metasd/WRLD3-03/wrld3-03.mdl".to_string()
    } else {
        first
    };
    let budget: usize = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(usize::MAX);
    run_real_model(&mdl_path, budget);
}
