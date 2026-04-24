// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Reproducer for the LTM compilation blow-up that prevents World3 from
//! simulating in the Simlin UI.  The failure path is:
//! `model.run()` -> `simulate(enableLtm=true)` ->
//! `simlin_sim_new(enable_ltm=true)` -> `set_project_ltm_enabled` then
//! `compile_project_incremental`.  Without the fix, LTM compilation enters
//! a pathological state (exponential DFS over every circuit in a dense
//! causal graph, producing gigabytes of intermediate state).  In WASM the
//! engine runs out of linear-memory headroom and rustc's panic=abort surfaces
//! as the infamous `RuntimeError: unreachable` inside the UI, blocking
//! sparkline rendering entirely.
//!
//! This test reproduces the cliff in plain Rust (no WASM) under a strict
//! wall-clock budget so regressions cannot silently sneak back in.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use simlin_engine::canonicalize;
use simlin_engine::db::{
    SimlinDb, causal_graph_from_element_edges, compile_project_incremental,
    model_element_causal_edges, set_project_ltm_enabled, sync_from_datamodel_incremental,
};
use simlin_engine::open_vensim;

fn load_wrld3() -> simlin_engine::datamodel::Project {
    let mdl_content = std::fs::read_to_string("../../test/metasd/WRLD3-03/wrld3-03.mdl").expect(
        "failed to read wrld3-03.mdl -- run tests from the repo root or simlin-engine crate",
    );
    open_vensim(&mdl_content).expect("open_vensim should parse wrld3-03.mdl without I/O errors")
}

/// World3 must be compilable with LTM enabled within a few seconds without
/// panicking or blowing memory.  We accept either `Ok` or an `Err` with a
/// diagnostic -- the UI's contract is only that LTM compilation finishes in
/// a reasonable time and does not crash the WASM module.
///
/// Budget is intentionally generous (60s) to avoid flakiness on slow CI
/// machines while still detecting the pre-fix behaviour, which runs for
/// many minutes and exhausts memory before making progress.
#[test]
fn wrld3_ltm_compilation_finishes_in_time() {
    let project = load_wrld3();

    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        set_project_ltm_enabled(&mut db, sync.project, true);

        // The key assertion: this call must not panic, not unreachable,
        // and must return within the budget below.  An `Err` is acceptable
        // -- the UI then falls back to a non-LTM simulation and still
        // renders sparklines.
        let result = compile_project_incremental(&db, sync.project, "main");
        let _ = tx.send(result.map(|_| ()));
    });

    let budget = Duration::from_secs(60);
    match rx.recv_timeout(budget) {
        Ok(_) => {
            // compile_project_incremental returned (Ok or Err) within the
            // budget.  Join the worker to surface any late panic.
            handle.join().expect("LTM compile thread panicked");
        }
        Err(mpsc::RecvTimeoutError::Timeout) => panic!(
            "LTM compilation for World3 did not finish within {:?}.  Pre-fix \
             regression detected: the CausalGraph DFS is running away.",
            budget
        ),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            // Worker panicked -- propagate the panic so the test fails with
            // a backtrace pointing at the offending line.
            handle.join().expect("LTM compile thread panicked");
        }
    }
}

/// Regression test for the retired `MAX_LTM_CIRCUITS` cap.  With the cap
/// removed, callers that ask for an uncapped enumeration (`usize::MAX`)
/// must receive the full 1,863,803 elementary circuits of the World3
/// element-level causal graph rather than a `TruncatedByBudget` signal or
/// a silently truncated result.
///
/// `MAX_LTM_SCC_NODES` remains the real backstop: it gates the downstream
/// synthetic-variable pipeline at `model_ltm_variables`.  Pure
/// enumeration stays uncapped so diagnostic tools and future stress tests
/// can measure the raw graph structure.
///
/// Gated with `#[ignore]` because Johnson's algorithm on WRLD3's 166-node
/// SCC takes ~30s in debug -- over the per-test budget and a meaningful
/// fraction of the 180s workspace cap.  The sibling
/// `wrld3_ltm_compilation_finishes_in_time` already protects the
/// every-push compilation path under its own 60s thread budget.  Run this
/// one on demand when changing enumeration logic:
///
///     cargo test --release -p simlin-engine --test wrld3_ltm_panic \
///         -- --ignored wrld3_element_level_enumeration_is_uncapped
#[test]
#[ignore]
fn wrld3_element_level_enumeration_is_uncapped() {
    let project = load_wrld3();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);

    let root_name = project
        .models
        .first()
        .map(|m| m.name.as_str())
        .expect("wrld3 must have at least one model");
    let root_canonical = canonicalize(root_name).into_owned();
    let source_model = sync
        .models
        .get(&root_canonical)
        .expect("root model must be in sync result")
        .source_model;

    let element_edges = model_element_causal_edges(&db, source_model, sync.project);
    let graph = causal_graph_from_element_edges(element_edges);

    let (_names, circuits) = graph
        .find_indexed_circuits_with_limit(usize::MAX)
        .expect("usize::MAX budget must not trip TruncatedByBudget");

    assert_eq!(
        circuits.len(),
        1_863_803,
        "wrld3-03 element-level enumeration count must match the \
         post-dedup Johnson's output measured by the 2026-04-18 \
         adversarial cap-lift validation"
    );
}
