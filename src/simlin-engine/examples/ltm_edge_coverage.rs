// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What fraction of a model's causal edges carry a LIVE link score?
//!
//! `examples/ltm_fragment_failures.rs` answers "which fragments fail and why".
//! It cannot answer the question that decides whether a failure MATTERS: a
//! failing fragment degrades to a constant 0, and
//! `ltm_finding::SearchGraph::from_edges` DROPS a zero-scored edge outright --
//! so a dead score on a *feed-forward* edge costs only that edge's attribution,
//! while a dead score on an edge inside a cycle makes every loop through it
//! undiscoverable. Counting failures without separating those two conflates a
//! cosmetic gap with a structural one.
//!
//! This harness joins three facts per variable-level causal edge:
//!   * is the edge inside a cycle (both endpoints in one non-trivial SCC of the
//!     ELEMENT graph -- the granularity LTM actually scores at)?
//!   * does the edge have any emitted link-score variable at all?
//!   * do all of that edge's link-score variables fail to compile?
//!
//! and reports the cross-tabulation, so "dead edges on cycles" is a number
//! rather than an impression. Bucketing by failure class (module-instance
//! endpoint vs. array-valued operand) says which open defect owns them.
//!
//! Usage:
//!   cargo run --release -p simlin-engine --example ltm_edge_coverage
//!   LTM_COV_MODEL=path/to/model.mdl cargo run --release ... --example ltm_edge_coverage
//!   LTM_COV_LIST=1 ...   # list every dead-on-cycle edge

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use simlin_engine::db::{
    SimlinDb, model_element_causal_edges, model_ltm_variables, set_project_ltm_enabled,
    sync_from_datamodel_incremental,
};
use simlin_engine::{open_vensim, open_xmile};

const LINK_PREFIX: &str = "$\u{205A}ltm\u{205A}link_score\u{205A}";
const ARROW: char = '\u{2192}';

/// A node's variable name: the element subscript stripped off.
///
/// The element graph names an arrayed node `var[elem]`; a link score names the
/// same node with or without the subscript depending on which side of the edge
/// carries the per-element pin. Comparing at variable granularity is what makes
/// the two namespaces joinable at all, and it is the right granularity for the
/// question here -- a defect that kills one element's score kills the class.
fn base_of(node: &str) -> &str {
    match node.find('[') {
        Some(i) => &node[..i],
        None => node,
    }
}

/// Split a link-score variable name into its `(from, to)` endpoint bases.
///
/// Returns `None` for a name that is not a link score. The `⁚via⁚{exit}` suffix
/// of an exhaustive-mode per-exit-port alias is stripped from the `to` side: it
/// names the same causal edge.
fn link_endpoints(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix(LINK_PREFIX)?;
    let (from, to) = rest.split_once(ARROW)?;
    let to = match to.find("\u{205A}via\u{205A}") {
        Some(i) => &to[..i],
        None => to,
    };
    Some((base_of(from).to_string(), base_of(to).to_string()))
}

/// Which open defect a failing fragment belongs to, keyed on the compiler
/// reason rather than on the equation body -- the reason is what #994 made
/// available, and it is the thing that partitions the failures cleanly.
fn failure_class(reason: &str) -> &'static str {
    if reason.contains("PREVIOUS requires a variable reference")
        || reason.contains("DimensionInScalarContext")
    {
        // A module-instance endpoint: the partial holds a `module·port` read
        // (or the target's un-rewritten arrayed body) in scalar context.
        "module-instance endpoint (#716)"
    } else if reason.contains("Cannot push view")
        || reason.contains("array-producing builtin outside AssignTemp")
        || reason.contains("Non-scalar StaticSubscript")
    {
        "array-valued operand (#995)"
    } else {
        "other"
    }
}

/// Tarjan's SCC, iterative (the element graph of a real model is deep enough
/// that a recursive walk is a stack-overflow risk, which under `panic = abort`
/// takes the process down).
fn sccs(nodes: &[String], adj: &HashMap<String, BTreeSet<String>>) -> HashMap<String, usize> {
    let index_of: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let succ: Vec<Vec<usize>> = nodes
        .iter()
        .map(|n| {
            adj.get(n)
                .map(|s| {
                    s.iter()
                        .filter_map(|t| index_of.get(t.as_str()).copied())
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect();

    let n = nodes.len();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut comp = vec![usize::MAX; n];
    let mut next_index = 0usize;
    let mut next_comp = 0usize;

    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        // (node, next successor to visit)
        let mut work: Vec<(usize, usize)> = vec![(root, 0)];
        index[root] = next_index;
        low[root] = next_index;
        next_index += 1;
        stack.push(root);
        on_stack[root] = true;

        while let Some(&mut (v, ref mut pi)) = work.last_mut() {
            if *pi < succ[v].len() {
                let w = succ[v][*pi];
                *pi += 1;
                if index[w] == usize::MAX {
                    index[w] = next_index;
                    low[w] = next_index;
                    next_index += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                if low[v] == index[v] {
                    while let Some(w) = stack.pop() {
                        on_stack[w] = false;
                        comp[w] = next_comp;
                        if w == v {
                            break;
                        }
                    }
                    next_comp += 1;
                }
                work.pop();
                if let Some(&(parent, _)) = work.last() {
                    low[parent] = low[parent].min(low[v]);
                }
            }
        }
    }

    nodes
        .iter()
        .enumerate()
        .map(|(i, name)| (name.clone(), comp[i]))
        .collect()
}

fn main() {
    let model_path = std::env::var("LTM_COV_MODEL")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl")
        });
    let list_dead = std::env::var("LTM_COV_LIST").is_ok();

    let contents = std::fs::read_to_string(&model_path).expect("read model");
    let datamodel = if model_path.extension().is_some_and(|e| e == "mdl") {
        open_vensim(&contents).expect("import vensim model")
    } else {
        open_xmile(&mut contents.as_bytes()).expect("import xmile model")
    };
    println!("model: {}", model_path.display());

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);

    let main_name = datamodel
        .models
        .iter()
        .find(|m| m.name == "main")
        .map(|m| m.name.clone())
        .unwrap_or_else(|| datamodel.models[0].name.clone());
    let source_model = sync.models[main_name.as_str()].source_model;

    let elem = model_element_causal_edges(&db, source_model, sync.project);
    let ltm = model_ltm_variables(&db, source_model, sync.project);
    println!("ltm mode: {:?}", ltm.mode);

    // Dump the element-level edges incident on nodes matching a needle, so the
    // element granularity an emitter would have to target is inspectable rather
    // than assumed.
    if let Ok(needle) = std::env::var("LTM_COV_EDGES") {
        println!("\n=== element edges incident on {needle:?} ===");
        for (from, tos) in elem.edges.iter().collect::<BTreeMap<_, _>>() {
            for to in tos.iter() {
                if from.contains(&needle) || to.contains(&needle) {
                    println!("  {from} -> {to}");
                }
            }
        }
        for var in ltm.vars.iter() {
            if var.name.contains(&needle) {
                println!("  score: {} dims={:?}", var.name, var.dimensions);
            }
        }
        // Every LTM diagnostic naming the needle -- the DECLINE warnings as well
        // as the compile failures. An edge with no score and no warning is a
        // silent gap; one with a warning is a decision, and the message says
        // which.
        println!("\n=== LTM diagnostics naming {needle:?} ===");
        for d in simlin_engine::db::collect_all_diagnostics(&db, sync.project).iter() {
            if let simlin_engine::db::DiagnosticError::Assembly(msg) = &d.error
                && msg.contains(&needle)
            {
                println!("  [{:?}] {msg}", d.severity);
            }
        }
    }

    // --- cycle membership, at element granularity ---------------------------
    let mut nodes: BTreeSet<String> = BTreeSet::new();
    for (from, tos) in elem.edges.iter() {
        nodes.insert(from.clone());
        for to in tos {
            nodes.insert(to.clone());
        }
    }
    let node_list: Vec<String> = nodes.iter().cloned().collect();
    let comp = sccs(&node_list, &elem.edges);
    let mut comp_size: HashMap<usize, usize> = HashMap::new();
    for c in comp.values() {
        *comp_size.entry(*c).or_insert(0) += 1;
    }

    // Variable-level edge -> is any of its element instances inside a cycle?
    let mut edge_cyclic: BTreeMap<(String, String), bool> = BTreeMap::new();
    let mut elem_edges = 0usize;
    let mut elem_edges_cyclic = 0usize;
    for (from, tos) in elem.edges.iter() {
        for to in tos {
            elem_edges += 1;
            let cf = comp.get(from);
            let ct = comp.get(to);
            let cyclic = match (cf, ct) {
                (Some(a), Some(b)) => a == b && (comp_size[a] > 1 || from == to),
                _ => false,
            };
            if cyclic {
                elem_edges_cyclic += 1;
            }
            let key = (base_of(from).to_string(), base_of(to).to_string());
            let e = edge_cyclic.entry(key).or_insert(false);
            *e |= cyclic;
        }
    }

    // --- which edges have scores, and are they alive? -----------------------
    let diagnostics = simlin_engine::db::collect_all_diagnostics(&db, sync.project);
    let mut failed: BTreeSet<String> = BTreeSet::new();
    let mut reasons: HashMap<String, String> = HashMap::new();
    for d in &diagnostics {
        let simlin_engine::db::DiagnosticError::Assembly(msg) = &d.error else {
            continue;
        };
        if !msg.contains("failed to compile") || d.model != main_name {
            continue;
        }
        if let Some(v) = &d.variable {
            failed.insert(v.clone());
            if let Some(r) = msg.split("Reason").nth(1) {
                reasons.insert(
                    v.clone(),
                    r.trim_start_matches([':', ' ']).trim().to_string(),
                );
            }
        }
    }

    // Per variable-level edge: (total scores, failing scores, classes seen).
    let mut edge_scores: BTreeMap<(String, String), (usize, usize, BTreeSet<&'static str>)> =
        BTreeMap::new();
    for var in ltm.vars.iter() {
        let Some(key) = link_endpoints(&var.name) else {
            continue;
        };
        let entry = edge_scores.entry(key).or_insert((0, 0, BTreeSet::new()));
        entry.0 += 1;
        if failed.contains(&var.name) {
            entry.1 += 1;
            let class = reasons
                .get(&var.name)
                .map(|r| failure_class(r))
                .unwrap_or("other");
            entry.2.insert(class);
        }
    }

    // --- the cross-tabulation ----------------------------------------------
    let mut rows: BTreeMap<(&'static str, &'static str), usize> = BTreeMap::new();
    let mut dead_on_cycle: BTreeMap<&'static str, Vec<(String, String)>> = BTreeMap::new();
    for (edge, cyclic) in edge_cyclic.iter() {
        let cyc = if *cyclic {
            "on a cycle"
        } else {
            "feed-forward"
        };
        let state = match edge_scores.get(edge) {
            None => "no score emitted",
            Some((_total, 0, _)) => "live score",
            Some((total, failing, _)) if failing == total => "ALL scores dead",
            Some(_) => "partially dead",
        };
        *rows.entry((cyc, state)).or_insert(0) += 1;
        if *cyclic && state != "live score" {
            let class = edge_scores
                .get(edge)
                .and_then(|(_, _, classes)| classes.iter().next().copied())
                .unwrap_or("no score emitted");
            dead_on_cycle.entry(class).or_default().push(edge.clone());
        }
    }

    println!(
        "\nelement graph: {} nodes, {elem_edges} edges",
        node_list.len()
    );
    println!("  element edges inside a cycle: {elem_edges_cyclic}");
    println!(
        "variable-level causal edges: {} ({} with at least one cyclic element instance)",
        edge_cyclic.len(),
        edge_cyclic.values().filter(|c| **c).count()
    );
    println!(
        "emitted link-score variables: {}",
        edge_scores.values().map(|(t, _, _)| t).sum::<usize>()
    );
    println!("  of which fail to compile:   {}", failed.len());

    println!("\n=== variable-level edges: cycle membership x score state ===");
    println!("  {:<14} {:<20} {:>8}", "position", "score state", "edges");
    for ((cyc, state), n) in &rows {
        println!("  {cyc:<14} {state:<20} {n:>8}");
    }

    println!("\n=== edges ON A CYCLE with no usable score, by failure class ===");
    if dead_on_cycle.is_empty() {
        println!("  (none)");
    }
    for (class, edges) in &dead_on_cycle {
        println!("  {:>6}  {class}", edges.len());
        let show = if list_dead { edges.len() } else { 6 };
        for (from, to) in edges.iter().take(show) {
            println!("            {from} -> {to}");
        }
        if edges.len() > show {
            println!("            ... {} more", edges.len() - show);
        }
    }
}
