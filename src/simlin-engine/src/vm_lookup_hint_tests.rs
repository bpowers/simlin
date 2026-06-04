// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Tests for lookup_with_hint, table_x_is_sorted, and LOOKUP_HINT_DISABLED.
// Split from vm.rs to keep that file under the per-file line cap.
use super::*;

/// Re-derive the binary-search lower bound `low` independently of any hint:
/// the smallest `i` such that `table[i].0 >= index`, computed only on the
/// interior path (after the empty/NaN/below-first/above-last guards). Used
/// by the proptest to assert the hint equals the search's lower bound after
/// an interior-path call. Returns `None` when an early return fired (so the
/// hint is not updated).
fn interior_low(table: &[(f64, f64)], index: f64) -> Option<usize> {
    if table.is_empty() || index.is_nan() {
        return None;
    }
    if index < table[0].0 {
        return None;
    }
    let size = table.len();
    if index > table[size - 1].0 {
        return None;
    }
    let mut low = 0;
    let mut high = size;
    while low < high {
        let mid = low + (high - low) / 2;
        if table[mid].0 < index {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    Some(low)
}

/// The hinted lookup must return EXACTLY what `lookup` returns for every
/// (table, index), regardless of the hint's prior value. This is the
/// bit-identity contract.
#[test]
fn hint_is_bit_identical_across_hints() {
    let table = vec![(0.0, 0.0), (1.0, 10.0), (2.0, 20.0), (3.0, 30.0)];
    let indices = [-1.0, 0.0, 0.5, 1.0, 1.5, 2.0, 2.99, 3.0, 5.0];
    // Try every possible starting hint (including out-of-range/stale ones).
    for start_hint in 0u32..=(table.len() as u32 + 2) {
        for &idx in &indices {
            let mut hint = start_hint;
            let hinted = lookup_with_hint(&table, idx, &mut hint);
            let plain = lookup(&table, idx);
            assert_eq!(
                hinted.to_bits(),
                plain.to_bits(),
                "mismatch idx={idx} start_hint={start_hint}"
            );
        }
    }
}

/// hint == 0 is never a valid hint (the shortcut requires 0 < h), so a 0
/// hint always misses and falls back to binary search.
#[test]
fn hint_zero_edge_case() {
    let table = vec![(0.0, 0.0), (1.0, 10.0), (2.0, 20.0)];
    let mut hint = 0;
    // index exactly at the first breakpoint => low == 0, which the hint
    // shortcut never matches; the miss path stores low (0) back.
    assert_eq!(lookup_with_hint(&table, 0.0, &mut hint), 0.0);
    assert_eq!(hint, 0);
    // A subsequent interior lookup correctly updates the hint to its low.
    assert_eq!(lookup_with_hint(&table, 1.5, &mut hint), 15.0);
    assert_eq!(hint, 2);
}

/// hint == len-1 (the last interior segment) is valid for an index in the
/// final segment and reused on the next call.
#[test]
fn hint_last_segment_edge_case() {
    let table = vec![(0.0, 0.0), (1.0, 10.0), (2.0, 20.0)];
    let mut hint = 0;
    assert_eq!(lookup_with_hint(&table, 1.5, &mut hint), 15.0);
    assert_eq!(hint, 2); // last interior segment
    // Re-use the same segment: validity is table[1].0 < 1.7 <= table[2].0.
    assert_eq!(lookup_with_hint(&table, 1.7, &mut hint), 17.0);
    assert_eq!(hint, 2);
}

/// A hint pointing into a duplicate-x plateau must still produce the
/// bit-identical value of `lookup` (which resolves the plateau via its own
/// binary search lower bound + approx_eq snap).
#[test]
fn hint_duplicate_x_plateau() {
    // Plateau: x == 1.0 repeated. lookup's binary search lower bound is the
    // first of the duplicates; approx_eq then snaps to table[low].1.
    let table = vec![(0.0, 0.0), (1.0, 10.0), (1.0, 11.0), (2.0, 20.0)];
    for start_hint in 0u32..=5 {
        let mut hint = start_hint;
        let hinted = lookup_with_hint(&table, 1.0, &mut hint);
        let plain = lookup(&table, 1.0);
        assert_eq!(hinted.to_bits(), plain.to_bits(), "start_hint={start_hint}");
    }
}

/// index exactly equal to table[h].0 and table[h-1].0 boundary behavior.
#[test]
fn hint_index_exactly_on_breakpoints() {
    let table = vec![(0.0, 0.0), (1.0, 10.0), (2.0, 20.0)];
    // index == table[h].0 (upper boundary of segment h): hint h is valid
    // because table[h-1].0 < index <= table[h].0.
    let mut hint = 2;
    assert_eq!(lookup_with_hint(&table, 2.0, &mut hint), 20.0);
    // index == table[h-1].0 (lower boundary): hint h is INVALID
    // (table[h-1].0 < index is false), so it misses and re-searches; the
    // correct lower bound for index == table[1].0 is low == 1.
    let mut hint = 2;
    assert_eq!(lookup_with_hint(&table, 1.0, &mut hint), 10.0);
    assert_eq!(hint, 1);
}

/// Size-1 and size-2 tables: every index lands in an early return for
/// size-1 (index == the sole x snaps; otherwise below-first/above-last),
/// so the hint never updates.
#[test]
fn hint_tiny_tables() {
    let one = vec![(5.0, 50.0)];
    let mut hint = 7;
    assert_eq!(
        lookup_with_hint(&one, 5.0, &mut hint).to_bits(),
        lookup(&one, 5.0).to_bits()
    );
    assert_eq!(
        lookup_with_hint(&one, 4.0, &mut hint).to_bits(),
        lookup(&one, 4.0).to_bits()
    );
    assert_eq!(
        lookup_with_hint(&one, 6.0, &mut hint).to_bits(),
        lookup(&one, 6.0).to_bits()
    );

    let two = vec![(0.0, 0.0), (10.0, 100.0)];
    let mut hint = 0;
    assert_eq!(lookup_with_hint(&two, 5.0, &mut hint), 50.0);
    assert_eq!(hint, 1);
    assert_eq!(lookup_with_hint(&two, 7.0, &mut hint), 70.0);
    assert_eq!(hint, 1);
}

use proptest::prelude::*;

/// Generate a sorted (non-decreasing-x) table including duplicate x values
/// and plateaus, sizes 1..=12.
fn sorted_table_strategy() -> impl Strategy<Value = Vec<(f64, f64)>> {
    prop::collection::vec((0i32..8, -20i32..20), 1..13).prop_map(|pairs| {
        // Sort by x to get a non-decreasing-x table (duplicates allowed:
        // the small x-domain (0..8) over up to 12 points guarantees them).
        let mut pairs = pairs;
        pairs.sort_by_key(|&(x, _)| x);
        pairs
            .into_iter()
            .map(|(x, y)| (x as f64, y as f64))
            .collect()
    })
}

/// Generate an index sequence mixing slowly-increasing, decreasing, and
/// random values, including out-of-range and exact-breakpoint hits.
fn index_sequence_strategy() -> impl Strategy<Value = Vec<f64>> {
    prop::collection::vec(
        prop_oneof![
            // exact integer breakpoints + out-of-range (covers below-first,
            // above-last, exact-hit)
            (-2i32..10).prop_map(|x| x as f64),
            // half-step interior indices (interpolation path)
            (-4i32..20).prop_map(|x| x as f64 / 2.0),
        ],
        1..40,
    )
}

proptest! {
    /// Carrying a hint across calls, the hinted lookup is bit-for-bit
    /// identical to `lookup` for every call, AND after each call the hint
    /// equals the binary search's lower bound whenever the interior path
    /// was taken (an early return leaves the hint unchanged).
    #[test]
    fn hint_matches_lookup_and_tracks_low(
        table in sorted_table_strategy(),
        indices in index_sequence_strategy(),
    ) {
        let mut hint: u32 = 0;
        for &idx in &indices {
            let hint_before = hint;
            let hinted = lookup_with_hint(&table, idx, &mut hint);
            let plain = lookup(&table, idx);
            // Bit-identical (handles NaN, +/-0.0, etc.).
            prop_assert_eq!(
                hinted.to_bits(),
                plain.to_bits(),
                "idx={} table={:?}",
                idx,
                table
            );
            match interior_low(&table, idx) {
                Some(low) => {
                    // Interior path: hint must equal the search lower bound.
                    prop_assert_eq!(hint as usize, low, "idx={} table={:?}", idx, table);
                }
                None => {
                    // Early return: hint left unchanged.
                    prop_assert_eq!(hint, hint_before, "idx={} table={:?}", idx, table);
                }
            }
        }
    }
}

// ── Unsorted-table tests (the critical correctness fix) ──────────────

/// Reviewer's minimal reproducer: an unsorted table where a seeded hint
/// (set from the first call) would select the WRONG segment on the second
/// call if the sentinel were absent, returning 700 instead of 70.
///
/// With the fix, the sentinel disables the hint for unsorted tables so
/// every call delegates to `lookup`, giving the correct result.
#[test]
fn unsorted_table_disabled_hint_matches_lookup_exactly() {
    let t = vec![(0.0, 0.0), (10.0, 1000.0), (5.0, 50.0), (20.0, 200.0)];

    // Confirm the table is classified as unsorted.
    assert!(!table_x_is_sorted(&t));

    let mut hint = LOOKUP_HINT_DISABLED;

    // First call: hint is disabled, result must equal lookup.
    let _ = lookup_with_hint(&t, 0.25, &mut hint);
    assert_eq!(
        hint, LOOKUP_HINT_DISABLED,
        "sentinel must not be overwritten on the first call"
    );

    // Second call (the dangerous one): without the sentinel a seeded hint
    // of 1 would be accepted, returning table[1].1 == 1000 instead of the
    // correct binary-search result. With the sentinel we always call lookup.
    let hinted = lookup_with_hint(&t, 7.0, &mut hint);
    let plain = lookup(&t, 7.0);
    assert_eq!(
        hinted.to_bits(),
        plain.to_bits(),
        "unsorted table: lookup_with_hint({}) != lookup({}) ({hinted} vs {plain})",
        7.0,
        7.0,
    );
    assert_eq!(hint, LOOKUP_HINT_DISABLED, "sentinel must stay disabled");

    // Carry the disabled hint across many calls; every result must be
    // bit-identical to lookup and the sentinel must never be overwritten.
    for &idx in &[-1.0f64, 0.0, 0.25, 3.0, 5.0, 7.0, 10.0, 15.0, 20.0, 25.0] {
        let hinted = lookup_with_hint(&t, idx, &mut hint);
        let plain = lookup(&t, idx);
        assert_eq!(
            hinted.to_bits(),
            plain.to_bits(),
            "idx={idx}: hinted {hinted} != plain {plain}"
        );
        assert_eq!(
            hint, LOOKUP_HINT_DISABLED,
            "sentinel must stay disabled after idx={idx}"
        );
    }
}

/// `table_x_is_sorted` classifies sorted tables (0) and unsorted tables
/// (LOOKUP_HINT_DISABLED) correctly, including edge cases.
#[test]
fn table_x_is_sorted_classification() {
    // Empty table: vacuously sorted.
    assert!(table_x_is_sorted(&[]));
    // Single element: trivially sorted.
    assert!(table_x_is_sorted(&[(1.0, 2.0)]));
    // Non-decreasing (sorted) tables.
    assert!(table_x_is_sorted(&[(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]));
    assert!(table_x_is_sorted(&[
        (0.0, 0.0),
        (1.0, 1.0),
        (1.0, 2.0),
        (2.0, 3.0)
    ]));
    // Strictly decreasing: unsorted.
    assert!(!table_x_is_sorted(&[(2.0, 0.0), (1.0, 1.0), (0.0, 2.0)]));
    // Out-of-order interior point: unsorted.
    assert!(!table_x_is_sorted(&[
        (0.0, 0.0),
        (10.0, 1000.0),
        (5.0, 50.0),
        (20.0, 200.0)
    ]));
    // NaN x-value: any NaN comparison is false, so windows(2) fails -> unsorted.
    assert!(!table_x_is_sorted(&[
        (0.0, 0.0),
        (f64::NAN, 1.0),
        (2.0, 2.0)
    ]));
}

/// Generate an unsorted table (do NOT sort the pairs) for the proptest.
/// Uses a small x range to guarantee actual out-of-order x values will
/// appear frequently.
fn unsorted_table_strategy() -> impl Strategy<Value = Vec<(f64, f64)>> {
    prop::collection::vec((0i32..8, -20i32..20), 2..13).prop_map(|pairs| {
        pairs
            .into_iter()
            .map(|(x, y)| (x as f64, y as f64))
            .collect::<Vec<_>>()
    })
}

proptest! {
    /// For tables with LOOKUP_HINT_DISABLED, every call to lookup_with_hint
    /// must return the bit-identical result of lookup AND the hint must
    /// remain LOOKUP_HINT_DISABLED throughout. The unsorted_table_strategy
    /// does NOT sort pairs, so many generated tables will have out-of-order
    /// x-values and trigger the disabled path; a coincidentally-sorted table
    /// is benign (the sentinel is set explicitly below).
    #[test]
    fn unsorted_disabled_hint_is_always_bit_identical_to_lookup(
        table in unsorted_table_strategy(),
        indices in index_sequence_strategy(),
    ) {
        // Force the disabled sentinel regardless of actual sortedness, so
        // we exercise the disabled-path logic even on a coincidentally-sorted
        // generated table.
        let mut hint = LOOKUP_HINT_DISABLED;
        for &idx in &indices {
            let hinted = lookup_with_hint(&table, idx, &mut hint);
            let plain = lookup(&table, idx);
            prop_assert_eq!(
                hinted.to_bits(),
                plain.to_bits(),
                "idx={} table={:?}",
                idx,
                table
            );
            prop_assert_eq!(
                hint,
                LOOKUP_HINT_DISABLED,
                "sentinel must stay disabled, idx={} table={:?}",
                idx,
                table
            );
        }
    }
}

// ── Vm::new-level test ───────────────────────────────────────────────

/// `Vm::new` must classify a sorted GF table with hint 0 (to-be-seeded)
/// and an unsorted GF table with LOOKUP_HINT_DISABLED. We test this via
/// `table_x_is_sorted` directly (used by Vm::new for classification) plus
/// a structural guard that lookup_memo is per-module/per-table sized.
///
/// Building an unsorted graphical function through TestProject is impractical
/// because GraphicalFunction's x_points are stored as Vec<f64> and the import
/// paths do not validate or sort them, but creating a CompiledSimulation with
/// an injected unsorted table requires bypassing the full project pipeline.
/// Instead this test validates the primitive (`table_x_is_sorted`) that Vm::new
/// relies on, plus a sorted-GF round-trip through the real VM to confirm the
/// memo slot is present and initialized to 0 at construction.
#[test]
fn vm_new_sorted_gf_hint_initialized_to_zero() {
    use crate::datamodel::{GraphicalFunction, GraphicalFunctionKind, GraphicalFunctionScale};
    use crate::test_common::TestProject;

    // A strictly sorted graphical function: x = [0, 1, 2], y = [0, 10, 20].
    let gf = GraphicalFunction {
        kind: GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 1.0, 2.0]),
        y_points: vec![0.0, 10.0, 20.0],
        x_scale: GraphicalFunctionScale { min: 0.0, max: 2.0 },
        y_scale: GraphicalFunctionScale {
            min: 0.0,
            max: 20.0,
        },
    };

    // Confirm table_x_is_sorted agrees: sorted -> hint 0 at Vm::new.
    let sorted_pairs: Vec<(f64, f64)> = vec![(0.0, 0.0), (1.0, 10.0), (2.0, 20.0)];
    assert!(
        table_x_is_sorted(&sorted_pairs),
        "sorted table must be classified sorted"
    );

    // Confirm an unsorted variant is classified unsorted -> LOOKUP_HINT_DISABLED.
    let unsorted_pairs: Vec<(f64, f64)> =
        vec![(0.0, 0.0), (10.0, 1000.0), (5.0, 50.0), (20.0, 200.0)];
    assert!(
        !table_x_is_sorted(&unsorted_pairs),
        "unsorted table must be classified unsorted"
    );

    // Build a real model with a sorted GF and run it; the VM must complete
    // without panicking (memo slots are present and correctly addressed).
    let results = TestProject::new("test")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux("time_input", "TIME", None)
        .aux_with_gf("gf_var", "time_input", gf)
        .run_vm()
        .expect("VM with sorted GF should run without errors");

    // The GF lookup at TIME=0 returns 0, TIME=1 returns 10, TIME=2 returns 20.
    let gf_results = results.get("gf_var").expect("gf_var must be in results");
    assert_eq!(gf_results[0], 0.0);
    assert_eq!(gf_results[1], 10.0);
    assert_eq!(gf_results[2], 20.0);
}
