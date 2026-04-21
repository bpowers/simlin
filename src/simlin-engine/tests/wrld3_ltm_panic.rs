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

use simlin_engine::db::{
    SimlinDb, compile_project_incremental, set_project_ltm_enabled, sync_from_datamodel_incremental,
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
