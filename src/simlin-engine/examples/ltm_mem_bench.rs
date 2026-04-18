// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Memory and timing benchmark for LTM circuit enumeration.
//!
//! Usage: `cargo run --release --example ltm_mem_bench -- [mdl] [budget]`
//!
//! Example:
//!   cargo run --release --example ltm_mem_bench -- \
//!       test/metasd/WRLD3-03/wrld3-03.mdl 10000000
//!
//! Reports nodes, edges, stocks, wall-clock time, circuits found, and peak
//! RSS as reported by `/proc/self/status` (VmPeak / VmHWM).  Designed as a
//! repeatable measurement harness for iterating on the LTM enumeration
//! algorithm without having to reason about the full salsa pipeline.

use std::collections::HashMap;
use std::fs;
use std::time::Instant;

use simlin_engine::common::{Canonical, Ident};
use simlin_engine::db::{
    SimlinDb, causal_graph_from_edges, model_causal_edges, sync_from_datamodel,
};
use simlin_engine::ltm::CausalGraph;
use simlin_engine::open_vensim;

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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mdl_path = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "test/metasd/WRLD3-03/wrld3-03.mdl".to_string());
    let budget: usize = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(usize::MAX);

    eprintln!("=== LTM memory/time benchmark ===");
    eprintln!("model:  {mdl_path}");
    eprintln!("budget: {budget}");
    print_mem("startup");

    let mdl = fs::read_to_string(&mdl_path).expect("read model file");
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
    let t0 = Instant::now();
    // Use the indexed API (names table + u32 paths) so the benchmark
    // reflects the post-H13 memory profile -- the legacy
    // `Vec<Vec<Ident<Canonical>>>` path still exists for callers that
    // need owned idents, but production LTM flows through the indexed
    // view exclusively.
    let result = graph.find_indexed_circuits_with_limit(budget);
    let elapsed = t0.elapsed();

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
    print_mem("done");
}
