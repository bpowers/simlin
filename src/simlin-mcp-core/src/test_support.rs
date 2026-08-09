// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Shared fixtures for the integration test suites.
//!
//! Exposed as `#[doc(hidden)]` behind the `test-support` feature so that
//! integration tests under `tests/` can import it without the fixtures being
//! compiled into a shipped binary.

/// The filesystem-backed `ProjectAccess` the integration suites run against.
///
/// This is the PRODUCTION impl, not a stand-in; the alias makes it explicit
/// at each import that the fixture and production are one thing. Never
/// replace it with an independent implementation: a hand-maintained test
/// double drifts from `fs_access::FileSystemAccess` at exactly the points
/// where the real one is non-trivial (the `.mdl` write rejection, the SD-AI
/// `relationships` regeneration on save), leaving every e2e test that writes
/// a file exercising a simpler function than the one that ships.
pub use crate::fs_access::FileSystemAccess as TestFileSystemAccess;

/// Build a native-JSON project whose causal graph is a single SCC of
/// `total_nodes` nodes (a stock, a flow, and `total_nodes - 2` chained auxes),
/// which trips the engine's `MAX_LTM_SCC_NODES = 50` auto-flip gate when
/// `total_nodes >= 51`. Compiling with LTM enabled emits the "auto-switched ...
/// to discovery mode" Warning diagnostic.
///
/// Chain: `cap_stock -> aux_{N-3} -> ... -> aux_0 -> cap_flow -> cap_stock`.
///
/// Shared by the `read_model` and `edit_model` integration suites, which both
/// need a model that auto-flips to discovery mode to exercise the GH #662
/// LTM-warning surfacing.
pub fn chain_scc_project_json(total_nodes: usize) -> serde_json::Value {
    let aux_count = total_nodes - 2;
    let auxiliaries: Vec<serde_json::Value> = (0..aux_count)
        .map(|i| {
            let equation = if i + 1 == aux_count {
                "cap_stock".to_string()
            } else {
                format!("aux_{}", i + 1)
            };
            serde_json::json!({
                "uid": (i + 10) as i64,
                "name": format!("aux_{i}"),
                "equation": equation,
            })
        })
        .collect();

    serde_json::json!({
        "name": "chain_scc",
        "simSpecs": {
            "startTime": 0.0,
            "endTime": 10.0,
            "dt": "1",
            "saveStep": 1.0,
            "method": "euler",
            "timeUnits": ""
        },
        "models": [{
            "name": "main",
            "stocks": [
                {"uid": 1, "name": "cap_stock", "initialEquation": "0",
                 "inflows": ["cap_flow"], "outflows": []}
            ],
            "flows": [
                {"uid": 2, "name": "cap_flow", "equation": "aux_0"}
            ],
            "auxiliaries": auxiliaries
        }]
    })
}
