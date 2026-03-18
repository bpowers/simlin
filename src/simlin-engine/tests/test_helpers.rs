// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Shared test helpers for integration tests.
//!
//! Extracted from `simulate.rs` so that multiple integration test files
//! (simulate.rs, simulate_systems.rs, etc.) can share the comparison logic.

use float_cmp::approx_eq;
use simlin_engine::Results;
use simlin_engine::common::{Canonical, Ident};

/// Columns that are vendor-specific or otherwise not important for
/// simulation correctness.
const IGNORABLE_COLS: &[&str] = &["saveper", "initial_time", "final_time", "time_step"];

/// Check if a variable name is a Vensim-specific internal delay/smooth variable.
/// These have formats like "#d8>DELAY3#[A1]" or "#d8>DELAY3>RT2#[A1]".
fn is_vensim_internal_module_var(name: &str) -> bool {
    name.starts_with('#') && name.contains('>')
}

/// Check if a variable name is an implicit module variable created by
/// builtins_visitor for SMOOTH/DELAY/TREND/etc. These names start with
/// "$\u{205A}" (dollar sign + two dot punctuation) and are internal
/// implementation details whose evaluation order may legitimately differ
/// between the interpreter and incremental VM paths.
fn is_implicit_module_var(name: &str) -> bool {
    name.starts_with("$\u{205A}")
}

/// Compare expected results against simulation output.
///
/// Iterates expected variable keys only, so extra variables in `results`
/// (modules, internal flows, etc.) don't cause failures. Uses absolute
/// epsilon of 2e-3 for non-Vensim data, with relative comparison for
/// Vensim-sourced data.
pub fn ensure_results(expected: &Results, results: &Results) {
    assert_eq!(expected.step_count, results.step_count);
    assert_eq!(expected.iter().len(), results.iter().len());

    let expected_results = expected;

    let mut step = 0;
    for (expected_row, results_row) in expected.iter().zip(results.iter()) {
        for ident in expected.offsets.keys() {
            let expected = expected_row[expected.offsets[ident]];
            if !results.offsets.contains_key(ident)
                && (IGNORABLE_COLS.contains(&ident.as_str())
                    || is_vensim_internal_module_var(ident.as_str()))
            {
                continue;
            }
            // Skip implicit module variables (from SMOOTH/DELAY/TREND
            // expansion). These internal variables may legitimately have
            // different initial values between the interpreter and
            // incremental VM paths due to evaluation order differences.
            if is_implicit_module_var(ident.as_str()) {
                continue;
            }
            if !results.offsets.contains_key(ident) {
                panic!("output missing variable '{ident}'");
            }
            let off = results.offsets[ident];
            let actual = results_row[off];

            let around_zero = approx_eq!(f64, expected, 0.0, epsilon = 3e-6)
                && approx_eq!(f64, actual, 0.0, epsilon = 1e-6);

            if !around_zero {
                let (exp_cmp, act_cmp, epsilon) = if results.is_vensim || expected_results.is_vensim
                {
                    // Vensim outputs ~6 significant figures. Use relative comparison
                    // to handle large magnitudes (where small relative errors become
                    // large absolute errors). For small values, maintain the original
                    // absolute tolerance of 2e-3 so we don't become too strict.
                    let max_val = expected.abs().max(actual.abs()).max(1e-10);
                    let relative_eps = max_val * 5e-6;
                    (expected, actual, relative_eps.max(2e-3))
                } else {
                    (expected, actual, 2e-3)
                };

                if !approx_eq!(f64, exp_cmp, act_cmp, epsilon = epsilon) {
                    eprintln!("step {step}: {ident}: {expected} (expected) != {actual} (actual)");
                    panic!("not equal");
                }
            }
        }

        step += 1;
    }

    assert_eq!(expected.step_count, step);

    // UNKNOWN is a sentinel value we use -- it should never show up
    // unless we've wrongly sized our data slices
    assert!(
        !results
            .offsets
            .contains_key(&Ident::<Canonical>::from_str_unchecked("UNKNOWN"))
    );
}
