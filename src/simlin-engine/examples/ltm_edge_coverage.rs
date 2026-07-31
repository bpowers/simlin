// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What fraction of a model's causal edges carry a LIVE link score?
//!
//! `examples/ltm_fragment_failures.rs` answers "which fragments fail and why".
//! It cannot answer the question that decides whether a failure MATTERS:
//! `ltm_finding::IndexedSearch::load_step_scores` DROPS a zero-scored edge from
//! the per-step discovery graph, so a dead score on a *feed-forward* edge costs
//! only that edge's attribution, while a dead score on an edge inside a cycle
//! makes every loop through it undiscoverable. Counting failures without
//! separating those two conflates a cosmetic gap with a structural one. (The
//! `SearchGraph::from_edges` twin is `#[cfg(test)]` -- a reference oracle, not
//! the production drop site.)
//!
//! This harness joins, per variable-level causal edge:
//!   * is the edge inside a cycle (both endpoints in one non-trivial SCC of the
//!     ELEMENT graph -- the granularity LTM actually scores at)?
//!   * does the edge have any emitted link-score variable at all?
//!   * is any of those scores NON-ZERO at some saved step?
//!
//! The third is the one that matters and the one a static view cannot supply.
//! A compiled fragment is not a usable score: it can read a constant 0 handed to
//! it by a helper fragment that failed beneath it, and a perfectly correct score
//! can simply be 0 along this trajectory. So the harness compiles the
//! LTM-enabled project, runs it, and reads the columns. Compile status is kept
//! as a fallback for a model that does not simulate, and the report says which
//! measure it used.
//!
//! GRANULARITY CAVEAT, deliberate. The cycle/score join is at VARIABLE
//! granularity, because a link-score name maps to element endpoints through the
//! same expansion `ltm_finding::parse_link_offsets` performs and duplicating
//! that here would be a second derivation to keep in step. A pair whose
//! feed-forward element is unusable while its cyclic element is live is
//! therefore indistinguishable from the reverse -- so that case is reported as
//! its own "partially live" state and never folded into the dead-on-cycle
//! totals, which makes those totals a sound LOWER BOUND rather than an inflated
//! headline.
//!
//! Usage:
//!   cargo run --release -p simlin-engine --example ltm_edge_coverage
//!   LTM_COV_MODEL=path/to/model.mdl cargo run --release ... --example ltm_edge_coverage
//!   LTM_COV_LIST=1 ...   # list every dead-on-cycle edge
//!   LTM_COV_EDGES=needle ...   # element edges + scores + diagnostics for a name

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

/// Which open defect a failing fragment belongs to.
///
/// The compiler reason alone does NOT partition these: "PREVIOUS requires a
/// variable reference" is emitted both when a module partial freezes the
/// target's un-rewritten arrayed body (#716) and whenever an array-valued
/// reference is frozen at all (#995) -- the latter is exactly what
/// `db::ltm_element_instance_tests::an_arrayed_capture_helpers_scores_compile`
/// pins as the residual class. Attributing every occurrence to #716 would point
/// follow-up work at the wrong defect.
///
/// The discriminator is the edge's SOURCE. A module instance or per-element
/// capture helper is synthetic and carries the reserved `$⁚` prefix; an ordinary
/// model variable does not. Only the synthetic-source case is #716's shape.
fn failure_class(reason: &str, from: &str) -> &'static str {
    let synthetic_source = from.starts_with('$');
    if reason.contains("PREVIOUS requires a variable reference")
        || reason.contains("DimensionInScalarContext")
    {
        if synthetic_source {
            "module-instance endpoint (#716)"
        } else {
            "array-valued operand frozen at PREVIOUS (#995)"
        }
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

    // RUNTIME liveness -- the measure that matches what discovery consumes.
    //
    // A compiled fragment is NOT the same as a usable score, in two directions
    // the static view cannot see: a score can compile and still read a constant
    // 0 supplied by a helper fragment that failed beneath it, and a perfectly
    // correct score can simply be 0 along this trajectory. `load_step_scores`
    // drops an edge whose score is 0 AT A STEP, so "is this column ever
    // non-zero" is the honest question. Compile the LTM-enabled project, run it,
    // and record which link-score variables are non-zero at some saved step.
    //
    // `None` means the run could not be produced (a model that does not
    // simulate); the report then falls back to the static view and says so,
    // rather than silently reporting compile status as liveness.
    let runtime_nonzero: Option<BTreeSet<String>> = (|| {
        let compiled =
            simlin_engine::db::compile_project_incremental(&db, sync.project, &main_name)
                .map_err(|e| {
                    eprintln!("note: LTM compile failed, runtime liveness unavailable: {e:?}")
                })
                .ok()?;
        let mut vm = simlin_engine::Vm::new(compiled)
            .map_err(|e| eprintln!("note: VM creation failed: {e:?}"))
            .ok()?;
        vm.run_to_end()
            .map_err(|e| eprintln!("note: simulation failed: {e:?}"))
            .ok()?;
        let results = vm.into_results();
        let mut live = BTreeSet::new();
        for (name, &offset) in results.offsets.iter() {
            if !name.as_str().starts_with(LINK_PREFIX) {
                continue;
            }
            let nonzero = (0..results.step_count).any(|step| {
                let v = results.data[step * results.step_size + offset];
                v.is_finite() && v != 0.0
            });
            if nonzero {
                live.insert(name.as_str().to_string());
            }
        }
        Some(live)
    })();

    /// Per variable-level edge: how many link scores it has, how many fail to
    /// compile, how many are non-zero at some saved step, and which failure
    /// classes were seen.
    #[derive(Default)]
    struct EdgeScores {
        total: usize,
        failing: usize,
        runtime_live: usize,
        classes: BTreeSet<&'static str>,
    }

    let mut edge_scores: BTreeMap<(String, String), EdgeScores> = BTreeMap::new();
    let mut failed_link_scores = 0usize;
    for var in ltm.vars.iter() {
        let Some(key) = link_endpoints(&var.name) else {
            continue;
        };
        let entry = edge_scores.entry(key.clone()).or_default();
        entry.total += 1;
        if failed.contains(&var.name) {
            failed_link_scores += 1;
            entry.failing += 1;
            let class = reasons
                .get(&var.name)
                .map(|r| failure_class(r, &key.0))
                .unwrap_or("other");
            entry.classes.insert(class);
        }
        if runtime_nonzero
            .as_ref()
            .is_some_and(|live| live.contains(&var.name))
        {
            entry.runtime_live += 1;
        }
    }

    // --- the cross-tabulation ----------------------------------------------
    let mut rows: BTreeMap<(&'static str, &'static str), usize> = BTreeMap::new();
    let mut runtime_rows: BTreeMap<(&'static str, &'static str), usize> = BTreeMap::new();
    let mut dead_on_cycle: BTreeMap<&'static str, Vec<(String, String)>> = BTreeMap::new();
    let mut compiled_but_zero_on_cycle: Vec<(String, String)> = Vec::new();
    for (edge, cyclic) in edge_cyclic.iter() {
        let cyc = if *cyclic {
            "on a cycle"
        } else {
            "feed-forward"
        };
        // TWO ORTHOGONAL AXES, deliberately not merged.
        //
        // DEFECT state answers "is this edge broken" and is the headline: a
        // score that fails to compile keeps its slot with no bytecode and reads
        // a constant 0, which is a bug. RUNTIME state answers "does discovery
        // see anything here", which is what `load_step_scores` actually keys
        // on -- but an identically-zero column is NOT evidence of a defect,
        // because an edge whose influence really is zero is correctly scored
        // zero. Folding the two would relabel every genuinely-inert edge as
        // broken and overstate the problem by an order of magnitude (2,319
        // feed-forward edges on C-LEARN are zero at runtime and perfectly fine).
        //
        // What the runtime axis IS good for: an edge that COMPILES yet is
        // identically zero on a cycle is where a hidden stub would hide -- a
        // score reading a constant 0 handed to it by a helper fragment that
        // failed beneath it. That population is reported separately below.
        //
        // "Partially" states are their own rows and never folded into the
        // totals: this join is at VARIABLE granularity (see the module doc), so
        // a pair whose feed-forward element is broken while its cyclic element
        // is fine is indistinguishable here from the reverse. Keeping them
        // separate makes the dead-on-cycle figure a sound LOWER BOUND.
        let defect_state = match edge_scores.get(edge) {
            None => "no score emitted",
            Some(e) if e.failing == 0 => "all scores compile",
            Some(e) if e.failing == e.total => "ALL scores dead",
            Some(_) => "partially dead",
        };
        *rows.entry((cyc, defect_state)).or_insert(0) += 1;
        if *cyclic && matches!(defect_state, "no score emitted" | "ALL scores dead") {
            let class = edge_scores
                .get(edge)
                .and_then(|e| e.classes.iter().next().copied())
                .unwrap_or("no score emitted");
            dead_on_cycle.entry(class).or_default().push(edge.clone());
        }
        if let (Some(e), true) = (edge_scores.get(edge), runtime_nonzero.is_some()) {
            let rt = if e.runtime_live > 0 {
                "non-zero at some step"
            } else if e.failing == e.total {
                "zero (every score failed to compile)"
            } else {
                "zero at every step (compiles)"
            };
            *runtime_rows.entry((cyc, rt)).or_insert(0) += 1;
            if *cyclic && e.runtime_live == 0 && e.failing == 0 {
                compiled_but_zero_on_cycle.push(edge.clone());
            }
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
        edge_scores.values().map(|e| e.total).sum::<usize>()
    );
    // Two DIFFERENT numbers, and conflating them overstates the first: `failed`
    // holds every failing LTM fragment in this model -- loop scores, composites,
    // pathways, helpers -- while only some of those are link scores.
    println!("  of which fail to compile:        {failed_link_scores}");
    println!("failing LTM fragments (all kinds): {}", failed.len());
    match &runtime_nonzero {
        Some(live) => println!(
            "link scores non-zero at some step: {} (runtime liveness available)",
            live.len()
        ),
        None => println!("runtime liveness UNAVAILABLE -- falling back to compile status"),
    }

    println!("\n=== DEFECT: cycle membership x whether the scores compile ===");
    println!("  {:<14} {:<32} {:>8}", "position", "score state", "edges");
    for ((cyc, state), n) in &rows {
        println!("  {cyc:<14} {state:<32} {n:>8}");
    }

    if !runtime_rows.is_empty() {
        println!("\n=== RUNTIME: cycle membership x whether any score is non-zero ===");
        println!(
            "  (an identically-zero column is what discovery drops, but it is NOT\n                by itself a defect -- an edge with no influence is correctly zero)"
        );
        println!(
            "  {:<14} {:<32} {:>8}",
            "position", "runtime state", "edges"
        );
        for ((cyc, state), n) in &runtime_rows {
            println!("  {cyc:<14} {state:<32} {n:>8}");
        }
        println!(
            "\n  edges ON A CYCLE that compile yet are zero at every step: {}",
            compiled_but_zero_on_cycle.len()
        );
        println!(
            "    (the population where a score stubbed by a FAILING HELPER beneath it\n                  would hide; #994 measured 0 failing helpers under a compiling parent on\n                  this corpus, so these are expected to be genuine zeros -- but the count\n                  is printed rather than assumed)"
        );
        for (from, to) in compiled_but_zero_on_cycle.iter().take(if list_dead {
            compiled_but_zero_on_cycle.len()
        } else {
            6
        }) {
            println!("      {from} -> {to}");
        }
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
