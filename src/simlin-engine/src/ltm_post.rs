// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Post-simulation computation of LTM relative loop scores.
//!
//! Historical context: exhaustive LTM used to emit a synthetic
//! `$⁚ltm⁚rel_loop_score⁚{id}` variable for every loop whose equation
//! normalized that loop's `loop_score` against the partition sum of
//! `|loop_score_j|`.  Emission was O(P²) text per partition (see
//! `docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md`) and
//! dominated compile memory for dense models.  Option B of the cap-lift
//! design plan moves the normalization here, executed post-simulation
//! against the O(P × save_steps) `loop_score` timeseries that the VM
//! already writes to `Results`.

use std::collections::{BTreeMap, HashMap};

use crate::common::{Canonical, Ident};
use crate::results::Results;

/// Build the canonical identifier of a loop's `loop_score` synthetic variable.
///
/// The constructed string already uses the canonical separators
/// (`$⁚ltm⁚loop_score⁚` with `⁚` = U+205A), so `Ident::new` does not
/// reallocate; the `Ident` wrapper is only there so callers can look the
/// series up in `Results::offsets` without further conversion.
pub(crate) fn loop_score_ident(loop_id: &str) -> Ident<Canonical> {
    let name = format!("$\u{205A}ltm\u{205A}loop_score\u{205A}{loop_id}");
    Ident::new(&name)
}

/// Compute per-loop, per-timestep relative loop scores from simulated
/// `loop_score` data.
///
/// For each loop whose `loop_score` series is present in `results`, the
/// returned value is:
///
/// ```text
/// rel_loop_score[i, t] = loop_score[i, t] / sum_j∈partition(|loop_score[j, t]|)
/// ```
///
/// `loop_partitions` maps each loop ID to its cycle-partition key (as
/// produced by `model_ltm_variables`).  Loops sharing a partition key
/// (including the `None` "no parent-level stock" group) form the
/// denominator.  This matches the grouping the (now-removed)
/// compile-time emitter used, but sources the mapping from salsa-cached
/// LTM compilation instead of rebuilding `Vec<Loop>` at each call site.
///
/// The denominator uses SAFEDIV-0 semantics: when
/// `sum_j(|loop_score_j, t|) == 0` the result is `0` rather than `NaN`.
/// Non-finite `loop_score` values (from upstream VM evaluation) propagate
/// through normal IEEE-754 arithmetic, matching the behaviour of the
/// removed SAFEDIV equation.
///
/// Loops whose `loop_score` is absent from `results` (e.g., because LTM
/// was disabled for that loop, or the model was compiled in discovery
/// mode) are omitted from the returned map.
pub fn compute_rel_loop_scores(
    results: &Results,
    loop_partitions: &HashMap<String, Option<usize>>,
) -> HashMap<String, Vec<f64>> {
    // Stable iteration order keeps partition grouping deterministic even
    // though the result map is itself unordered; callers that diff
    // timeseries across runs benefit from the predictable emit order.
    let mut loop_ids: Vec<&String> = loop_partitions.keys().collect();
    loop_ids.sort();

    let offsets: Vec<Option<usize>> = loop_ids
        .iter()
        .map(|id| results.offsets.get(&loop_score_ident(id)).copied())
        .collect();

    // BTreeMap rather than HashMap so partition-group iteration is
    // deterministic across runs.  Per-group summation is not
    // associative in IEEE-754 float arithmetic, so HashMap's
    // hash-randomized iteration would otherwise allow bit-for-bit
    // drift in the computed denominator between runs on the same
    // input — invisible to the existing tests (tolerances 1e-10 in the
    // proptest, 0.05 in the integration suite) but observable to
    // diffing consumers and undesirable in a salsa-cached pipeline.
    let mut partition_groups: BTreeMap<Option<usize>, Vec<usize>> = BTreeMap::new();
    for (i, id) in loop_ids.iter().enumerate() {
        let key = loop_partitions.get(*id).copied().unwrap_or(None);
        partition_groups.entry(key).or_default().push(i);
    }

    // One output series per loop, parallel to `loop_ids`.  Loops without
    // a known offset get an empty Vec so we can skip them when
    // assembling the final map.
    let mut series: Vec<Vec<f64>> = offsets
        .iter()
        .map(|o| {
            if o.is_some() {
                Vec::with_capacity(results.step_count)
            } else {
                Vec::new()
            }
        })
        .collect();

    for row in results.iter() {
        for indices in partition_groups.values() {
            let denom: f64 = indices
                .iter()
                .filter_map(|&i| offsets[i].map(|off| row[off].abs()))
                .sum();

            for &i in indices {
                let Some(off) = offsets[i] else { continue };
                let num = row[off];
                let val = if denom == 0.0 { 0.0 } else { num / denom };
                series[i].push(val);
            }
        }
    }

    let mut out: HashMap<String, Vec<f64>> = HashMap::with_capacity(loop_ids.len());
    for (i, id) in loop_ids.iter().enumerate() {
        if offsets[i].is_some() {
            out.insert((*id).clone(), std::mem::take(&mut series[i]));
        }
    }
    out
}

/// Compute per-timestep, per-element relative loop scores for A2A
/// arrayed loops.
///
/// [`compute_rel_loop_scores`] collapses every loop's `loop_score` to
/// slot 0 — correct and matches pre-PR FFI semantics (which also
/// returned a scalar series) for consumers that only needed the
/// first-element view.  But pre-PR's *compile-time* `rel_loop_score`
/// synthetic variables were genuinely per-element for A2A loops, and
/// callers that want per-element normalization (e.g., pysimlin users
/// reading dimension-aware importance series, or a future FFI that
/// exposes arrayed loop analysis) need a production path that
/// reproduces the same math.
///
/// Returns a flat `Vec<f64>` per loop id of length
/// `step_count * max_slots`, where `max_slots` is the largest slot
/// count among the loops sharing that partition group.  The value at
/// step `s`, element `k` is at index `s * max_slots + k`.  Scalar
/// loops in a mixed partition broadcast their single value across
/// every element slot, matching the behaviour of the pre-PR compile-
/// time emitter.
///
/// `n_slots_by_loop` maps each loop id to its element count.  Missing
/// entries or a count of 1 are treated as scalar.  The denominator at
/// element `k` is `sum_j |loop_score_j[k_j]|` where `k_j = k` for
/// arrayed loops and `k_j = 0` for scalar ones (broadcast semantics).
pub fn compute_rel_loop_scores_per_element(
    results: &Results,
    loop_partitions: &HashMap<String, Option<usize>>,
    n_slots_by_loop: &HashMap<String, usize>,
) -> HashMap<String, Vec<f64>> {
    // Sort ids so partition groups are deterministic (matches the
    // scalar variant's rationale).
    let mut loop_ids: Vec<&String> = loop_partitions.keys().collect();
    loop_ids.sort();

    let offsets: Vec<Option<usize>> = loop_ids
        .iter()
        .map(|id| results.offsets.get(&loop_score_ident(id)).copied())
        .collect();
    let slot_counts: Vec<usize> = loop_ids
        .iter()
        .map(|id| n_slots_by_loop.get(*id).copied().unwrap_or(1).max(1))
        .collect();

    let mut partition_groups: BTreeMap<Option<usize>, Vec<usize>> = BTreeMap::new();
    for (i, id) in loop_ids.iter().enumerate() {
        let key = loop_partitions.get(*id).copied().unwrap_or(None);
        partition_groups.entry(key).or_default().push(i);
    }

    // Per-group max_slots -- the stride used for both the numerator
    // and denominator walks.  Scalar-only groups trivially stride 1
    // and produce output identical to `compute_rel_loop_scores`.
    let group_max_slots: BTreeMap<Option<usize>, usize> = partition_groups
        .iter()
        .map(|(part, indices)| {
            let max = indices
                .iter()
                .map(|&i| slot_counts[i])
                .max()
                .unwrap_or(1)
                .max(1);
            (*part, max)
        })
        .collect();

    let mut series: Vec<Vec<f64>> = offsets
        .iter()
        .enumerate()
        .map(|(i, o)| {
            if o.is_some() {
                let key = loop_partitions.get(loop_ids[i]).copied().unwrap_or(None);
                let max_slots = group_max_slots.get(&key).copied().unwrap_or(1);
                vec![0.0_f64; results.step_count * max_slots]
            } else {
                Vec::new()
            }
        })
        .collect();

    for (step, row) in results.iter().enumerate() {
        for (part_key, indices) in &partition_groups {
            let max_slots = group_max_slots.get(part_key).copied().unwrap_or(1);
            for k in 0..max_slots {
                // Resolve per-loop element index once.  A loop with
                // slot_count == 1 broadcasts its single value to every
                // element; a loop with slot_count > 1 uses `k`
                // directly, but only when `k < slot_count` -- partition
                // membership keys on stock SCCs, so a single partition
                // can legitimately contain loops of mixed arity (e.g.,
                // a Region loop and a Region x Age loop).  The
                // mixed-arity loop has no value at elements beyond its
                // own length, so it drops out of both the denominator
                // sum and the numerator write for those k.
                let elem_index = |i: usize| -> Option<usize> {
                    let slots = slot_counts[i];
                    if slots > 1 {
                        if k < slots { Some(k) } else { None }
                    } else {
                        Some(0)
                    }
                };

                let denom: f64 = indices
                    .iter()
                    .filter_map(|&i| {
                        let elem = elem_index(i)?;
                        offsets[i].map(|off| row[off + elem].abs())
                    })
                    .sum();
                for &i in indices {
                    let Some(off) = offsets[i] else { continue };
                    let Some(elem) = elem_index(i) else { continue };
                    let num = row[off + elem];
                    let val = if denom == 0.0 { 0.0 } else { num / denom };
                    series[i][step * max_slots + k] = val;
                }
            }
        }
    }

    let mut out: HashMap<String, Vec<f64>> = HashMap::with_capacity(loop_ids.len());
    for (i, id) in loop_ids.iter().enumerate() {
        if offsets[i].is_some() {
            out.insert((*id).clone(), std::mem::take(&mut series[i]));
        }
    }
    out
}

/// Compute the per-timestep denominator series
/// `sum_j(|loop_score_j, t|)` for a single cycle partition.
///
/// Companion to [`compute_rel_loop_scores`] for callers that only need
/// one loop's normalized series at a time (the `libsimlin` single-loop
/// FFI) or that want to amortize denominator work across many
/// loop-level queries via partition-scoped caching.  Loops absent from
/// `results.offsets` contribute 0 to the sum (they are treated as if
/// LTM never wrote a `loop_score` for them, matching
/// `compute_rel_loop_scores`' skip behaviour).
///
/// Returns a `Vec<f64>` of length `results.step_count`.
pub fn compute_partition_denominator(
    results: &Results,
    loop_partitions: &HashMap<String, Option<usize>>,
    partition: Option<usize>,
) -> Vec<f64> {
    // Walk all loops keyed to this partition; skip ones whose
    // `loop_score` never made it into `results` (unlikely in practice
    // because `loop_partitions` is populated from the same
    // `LtmVariablesResult` that drove emission, but
    // `compute_rel_loop_scores` treats the mismatch as "skip", so do
    // the same here for parity).
    //
    // Sort the loop IDs before collecting offsets so the per-timestep
    // float summation walks indices in a stable order.  IEEE-754 sums
    // are non-associative, and the full-pass helper
    // `compute_rel_loop_scores` already sorts its loop IDs to pin the
    // denominator bit-for-bit across runs; without the same sort here
    // the `libsimlin` FFI single-loop path could return a
    // slightly different series than the full-pass path for the same
    // inputs.  The sort is O(partition_size * log partition_size) per
    // distinct partition and happens once (the result is cached at
    // the FFI layer), so the cost is negligible.
    let mut ids_in_partition: Vec<&String> = loop_partitions
        .iter()
        .filter_map(|(id, p)| if *p == partition { Some(id) } else { None })
        .collect();
    ids_in_partition.sort();
    let offsets: Vec<usize> = ids_in_partition
        .iter()
        .filter_map(|id| results.offsets.get(&loop_score_ident(id)).copied())
        .collect();

    let mut denom = Vec::with_capacity(results.step_count);
    for row in results.iter() {
        let sum: f64 = offsets.iter().map(|&off| row[off].abs()).sum();
        denom.push(sum);
    }
    denom
}

/// Compute the relative-loop-score series for a single loop given a
/// precomputed partition denominator.
///
/// Returns `None` if `loop_id`'s `loop_score` is absent from
/// `results.offsets` (LTM was not enabled for this loop, or the
/// simulation auto-flipped to discovery mode).  `denominator` must be a
/// slice of length `results.step_count` produced by
/// [`compute_partition_denominator`] for the same partition as `loop_id`.
///
/// SAFEDIV-0 semantics match [`compute_rel_loop_scores`]: a zero
/// denominator at time `t` yields `0.0`, not `NaN`.
pub fn compute_rel_loop_score_for_id(
    results: &Results,
    loop_id: &str,
    denominator: &[f64],
) -> Option<Vec<f64>> {
    let offset = results.offsets.get(&loop_score_ident(loop_id)).copied()?;
    debug_assert_eq!(
        denominator.len(),
        results.step_count,
        "denominator length must match results step_count"
    );
    let mut series = Vec::with_capacity(results.step_count);
    for (row_idx, row) in results.iter().enumerate() {
        let num = row[offset];
        let denom = denominator[row_idx];
        let val = if denom == 0.0 { 0.0 } else { num / denom };
        series.push(val);
    }
    Some(series)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamodel::{Dt, SimMethod, SimSpecs};
    use crate::results::Specs;
    use proptest::prelude::*;

    /// Build a minimal `Results` from a list of `(loop_id, series)` pairs.
    /// The data layout matches the VM's: row-major, one chunk per saved step,
    /// with column 0 reserved for `time`.
    fn make_results_for_loops(pairs: &[(&str, &[f64])]) -> Results {
        assert!(!pairs.is_empty(), "need at least one loop series");
        let step_count = pairs[0].1.len();
        for (id, ser) in pairs.iter() {
            assert_eq!(
                ser.len(),
                step_count,
                "series for loop '{id}' must match the first series length"
            );
        }
        let step_size = pairs.len() + 1;
        let mut data = vec![0.0_f64; step_count * step_size];
        let mut offsets: HashMap<Ident<Canonical>, usize> = HashMap::new();
        offsets.insert(Ident::new("time"), 0);
        for (i, (id, _)) in pairs.iter().enumerate() {
            offsets.insert(loop_score_ident(id), i + 1);
        }
        for (step, row) in data.chunks_mut(step_size).enumerate() {
            row[0] = step as f64;
            for (i, (_, ser)) in pairs.iter().enumerate() {
                row[i + 1] = ser[step];
            }
        }

        let sim_specs = SimSpecs {
            start: 0.0,
            stop: (step_count.saturating_sub(1)) as f64,
            dt: Dt::Dt(1.0),
            save_step: None,
            sim_method: SimMethod::Euler,
            time_units: None,
        };

        Results {
            offsets,
            data: data.into_boxed_slice(),
            step_size,
            step_count,
            specs: Specs::from(&sim_specs),
            is_vensim: false,
        }
    }

    /// Build a `loop_partitions` mapping directly from `(loop_id, partition)` pairs.
    /// This matches the shape produced by `model_ltm_variables` at the call site.
    fn mapping(pairs: &[(&str, Option<usize>)]) -> HashMap<String, Option<usize>> {
        pairs
            .iter()
            .map(|(id, p)| ((*id).to_string(), *p))
            .collect()
    }

    /// Inlined reference implementation of the SAFEDIV formula previously
    /// emitted by `generate_relative_loop_score_equation`.
    ///
    /// This is intentionally a naive, per-timestep computation: the proptest
    /// compares against it to catch any numeric divergence from the old
    /// compile-time behaviour.
    fn reference_rel_loop_scores(
        loop_ids: &[String],
        loop_partitions: &HashMap<String, Option<usize>>,
        series: &[Vec<f64>],
    ) -> Vec<Vec<f64>> {
        let step_count = series.first().map(|s| s.len()).unwrap_or(0);
        // Mirror production's BTreeMap so the proptest's float-sum
        // order matches exactly — otherwise the random-input test could
        // drift past the 1e-10 tolerance even though both sides compute
        // "the same" math.
        let mut groups: BTreeMap<Option<usize>, Vec<usize>> = BTreeMap::new();
        for (i, id) in loop_ids.iter().enumerate() {
            let key = loop_partitions.get(id).copied().unwrap_or(None);
            groups.entry(key).or_default().push(i);
        }
        let mut out: Vec<Vec<f64>> = (0..loop_ids.len())
            .map(|_| Vec::with_capacity(step_count))
            .collect();
        // `t` is an index into every per-loop series simultaneously, so
        // the range-based form is clearer than an iterator over one series.
        #[allow(clippy::needless_range_loop)]
        for t in 0..step_count {
            for indices in groups.values() {
                let denom: f64 = indices.iter().map(|&i| series[i][t].abs()).sum();
                for &i in indices {
                    let num = series[i][t];
                    let val = if denom == 0.0 { 0.0 } else { num / denom };
                    out[i].push(val);
                }
            }
        }
        out
    }

    #[test]
    fn two_loops_single_partition_normalizes() {
        // Two loops sharing partition 0.
        // rel[i, t] = ls[i, t] / (|ls[0, t]| + |ls[1, t]|).
        let series_a = &[1.0, 2.0, -4.0][..];
        let series_b = &[3.0, -4.0, 0.0][..];
        let results = make_results_for_loops(&[("A", series_a), ("B", series_b)]);
        let partitions = mapping(&[("A", Some(0)), ("B", Some(0))]);

        let scored = compute_rel_loop_scores(&results, &partitions);

        let rel_a = scored.get("A").expect("loop A should have a series");
        let rel_b = scored.get("B").expect("loop B should have a series");

        // t=0: denom = 1 + 3 = 4; rel_a = 0.25, rel_b = 0.75.
        assert!((rel_a[0] - 0.25).abs() < 1e-12);
        assert!((rel_b[0] - 0.75).abs() < 1e-12);
        // t=1: denom = 2 + 4 = 6; rel_a = 2/6, rel_b = -4/6.
        assert!((rel_a[1] - (2.0 / 6.0)).abs() < 1e-12);
        assert!((rel_b[1] - (-4.0 / 6.0)).abs() < 1e-12);
        // t=2: denom = 4 + 0 = 4; rel_a = -1, rel_b = 0.
        assert!((rel_a[2] - (-1.0)).abs() < 1e-12);
        assert!((rel_b[2]).abs() < 1e-12);
    }

    #[test]
    fn zero_denominator_yields_zero() {
        // Single loop whose loop_score is identically zero: without the
        // SAFEDIV-0 guard this would produce NaN.
        let series = &[0.0, 0.0, 0.0][..];
        let results = make_results_for_loops(&[("only", series)]);
        let partitions = mapping(&[("only", Some(0))]);

        let scored = compute_rel_loop_scores(&results, &partitions);
        let rel = scored.get("only").expect("loop should have a series");
        for (t, v) in rel.iter().enumerate() {
            assert_eq!(*v, 0.0, "SAFEDIV-0 should yield 0 at t={t}, got {v}");
        }
    }

    #[test]
    fn distinct_partitions_do_not_share_denominator() {
        // Two loops in separate partitions should each normalize against
        // only themselves, producing ±1 (except at zero) regardless of
        // the other loop's magnitude.
        let series_a = &[2.0, -5.0][..];
        let series_b = &[10.0, 0.0][..];
        let results = make_results_for_loops(&[("A", series_a), ("B", series_b)]);
        let partitions = mapping(&[("A", Some(0)), ("B", Some(1))]);

        let scored = compute_rel_loop_scores(&results, &partitions);
        let rel_a = scored.get("A").unwrap();
        let rel_b = scored.get("B").unwrap();

        assert!((rel_a[0] - 1.0).abs() < 1e-12);
        assert!((rel_a[1] - (-1.0)).abs() < 1e-12);
        assert!((rel_b[0] - 1.0).abs() < 1e-12);
        assert_eq!(
            rel_b[1], 0.0,
            "SAFEDIV-0 when loop_score = 0 in its own partition"
        );
    }

    #[test]
    fn missing_loop_score_is_omitted() {
        // Loop "A" has a series; loop "B" does not (offset lookup fails).
        // The returned map should only contain "A".
        let results = make_results_for_loops(&[("A", &[1.0, 2.0][..])]);
        let partitions = mapping(&[("A", Some(0)), ("B", Some(0))]);

        let scored = compute_rel_loop_scores(&results, &partitions);
        assert!(scored.contains_key("A"));
        assert!(
            !scored.contains_key("B"),
            "loops without a loop_score offset must be omitted"
        );
    }

    #[test]
    fn nan_loop_score_propagates_without_panic() {
        // Non-finite upstream values must flow through normal IEEE-754
        // arithmetic (the documented contract).  A panic or debug-assert
        // on NaN would be a subtle regression because the exhaustive
        // SAFEDIV equation silently propagated NaN via arithmetic.
        let nan = f64::NAN;
        let series_a = &[nan, 2.0][..];
        let series_b = &[1.0, 3.0][..];
        let results = make_results_for_loops(&[("A", series_a), ("B", series_b)]);
        let partitions = mapping(&[("A", Some(0)), ("B", Some(0))]);

        let scored = compute_rel_loop_scores(&results, &partitions);
        let rel_a = scored.get("A").unwrap();
        let rel_b = scored.get("B").unwrap();

        // t=0: denom = |NaN| + |1| = NaN; NaN/NaN = NaN for both loops.
        assert!(rel_a[0].is_nan(), "NaN numerator yields NaN result");
        assert!(rel_b[0].is_nan(), "NaN denominator yields NaN result");
        // t=1: well-defined; denom = 2 + 3 = 5.
        assert!((rel_a[1] - 0.4).abs() < 1e-12);
        assert!((rel_b[1] - 0.6).abs() < 1e-12);
    }

    #[test]
    fn unpartitioned_loops_share_default_group() {
        // Loops with `None` partition (no parent-level stock) should share
        // a single default group, matching the old compile-time emitter's
        // grouping of `partition_for_loop` -> `None` loops.
        let series_a = &[3.0][..];
        let series_b = &[1.0][..];
        let results = make_results_for_loops(&[("A", series_a), ("B", series_b)]);
        let partitions = mapping(&[("A", None), ("B", None)]);

        let scored = compute_rel_loop_scores(&results, &partitions);
        let rel_a = scored.get("A").unwrap();
        let rel_b = scored.get("B").unwrap();
        // Shared denom of 3 + 1 = 4.
        assert!((rel_a[0] - 0.75).abs() < 1e-12);
        assert!((rel_b[0] - 0.25).abs() < 1e-12);
    }

    #[test]
    fn partition_denominator_matches_full_pass_sum() {
        // The per-partition primitive must produce the same per-step
        // sum the full pass uses internally.  If it drifts, the
        // single-loop FFI would silently compute wrong rel_scores.
        let series_a = &[1.0, 2.0, -4.0][..];
        let series_b = &[3.0, -4.0, 0.0][..];
        let results = make_results_for_loops(&[("A", series_a), ("B", series_b)]);
        let partitions = mapping(&[("A", Some(0)), ("B", Some(0))]);

        let denom = compute_partition_denominator(&results, &partitions, Some(0));
        assert_eq!(denom.len(), 3);
        // t=0: |1| + |3| = 4.
        assert!((denom[0] - 4.0).abs() < 1e-12);
        // t=1: |2| + |-4| = 6.
        assert!((denom[1] - 6.0).abs() < 1e-12);
        // t=2: |-4| + |0| = 4.
        assert!((denom[2] - 4.0).abs() < 1e-12);

        // Loops in a different partition contribute nothing.
        let empty = compute_partition_denominator(&results, &partitions, Some(99));
        assert_eq!(empty, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn per_id_rel_loop_score_matches_full_pass() {
        // Bit-for-bit parity with `compute_rel_loop_scores` is the
        // load-bearing contract that lets the `libsimlin` FFI swap from
        // the full cache to the denominator-only cache without changing
        // observed output.  Exercise a mix of positive / negative /
        // zero values and multiple partitions to cover the SAFEDIV-0
        // and per-partition scoping branches.
        let series_a = &[1.0, 2.0, -4.0, 0.0][..];
        let series_b = &[3.0, -4.0, 0.0, 0.0][..];
        let series_c = &[7.0, 7.0, 7.0, 7.0][..];
        let results = make_results_for_loops(&[("A", series_a), ("B", series_b), ("C", series_c)]);
        let partitions = mapping(&[("A", Some(0)), ("B", Some(0)), ("C", Some(1))]);

        let full = compute_rel_loop_scores(&results, &partitions);

        for id in ["A", "B", "C"] {
            let partition = partitions.get(id).copied().unwrap();
            let denom = compute_partition_denominator(&results, &partitions, partition);
            let streamed = compute_rel_loop_score_for_id(&results, id, &denom)
                .unwrap_or_else(|| panic!("loop '{id}' must have a computed series"));
            let cached = full
                .get(id)
                .unwrap_or_else(|| panic!("loop '{id}' missing from full pass"));
            assert_eq!(
                &streamed, cached,
                "streamed rel_loop_score for '{id}' diverged from full-pass result"
            );
        }
    }

    #[test]
    fn per_id_rel_loop_score_denominator_is_order_independent() {
        // Reviewer iter-18 P3 / iter-19 nit: the streamed denominator
        // must sum loops in the same deterministic order the full-
        // pass helper does.  `compute_rel_loop_scores` sorts loop IDs
        // by name before summing; `compute_partition_denominator`
        // must do the same.  IEEE-754 non-associativity lets
        // `a + b + c` differ by a ULP from `c + a + b` when values
        // are close in magnitude (`1e16 + 1e16 + 1.0` is *bit-
        // identical* to `1.0 + 1e16 + 1e16` because `1.0` rounds
        // away, so a widely-spaced test passes even without the
        // sort).
        //
        // Use `[0.1, 0.2, ..., 0.7]` magnitudes on 7 partitioned
        // loops.  Rust's `HashMap` iterates in hash-bucket order, so
        // within a single process two `HashMap<String, ...>`
        // instances with the *same* default `RandomState` seed
        // iterate the same keys in the same order regardless of
        // insertion sequence -- testing with reversed insertion
        // orders alone does not reliably surface a missing sort.
        // Construct 8 `HashMap`s with *freshly seeded*
        // `RandomState`s instead, so their bucket layouts and
        // therefore iteration orders differ.  If
        // `compute_partition_denominator` sorts internally, every
        // trial returns the bit-identical denominator.  If it does
        // not, different hash seeds produce different sum orders
        // and at least one pair diverges, failing the assert.
        use std::collections::hash_map::RandomState;
        let seven_series: [(&str, &[f64]); 7] = [
            ("aaa_01", &[0.1_f64, 0.1][..]),
            ("bbb_02", &[0.2_f64, 0.2][..]),
            ("ccc_03", &[0.3_f64, 0.3][..]),
            ("ddd_04", &[0.4_f64, 0.4][..]),
            ("eee_05", &[0.5_f64, 0.5][..]),
            ("fff_06", &[0.6_f64, 0.6][..]),
            ("ggg_07", &[0.7_f64, 0.7][..]),
        ];
        let results = make_results_for_loops(&seven_series);

        // 16 trials: each `HashMap::with_hasher(RandomState::new())`
        // gets a fresh random seed, so iteration orders differ with
        // overwhelmingly high probability.  If the function sorts
        // internally, all 16 trials produce bit-identical
        // denominators; otherwise the chance that a buggy version
        // coincidentally produces the same iteration order across
        // all 16 trials is astronomically small (verified
        // empirically at ~93% catch rate with 8 trials; doubling
        // pushes it to >99%).
        let mut denoms: Vec<u64> = Vec::new();
        for _ in 0..16 {
            let mut map: HashMap<String, Option<usize>> = HashMap::with_hasher(RandomState::new());
            for (id, _) in &seven_series {
                map.insert((*id).to_string(), Some(0));
            }
            let denom = compute_partition_denominator(&results, &map, Some(0));
            assert_eq!(denom.len(), 2);
            denoms.push(denom[0].to_bits());
        }
        let first = denoms[0];
        for (i, bits) in denoms.iter().enumerate() {
            assert_eq!(
                *bits, first,
                "trial {i}: compute_partition_denominator must be hash-seed independent \
                 (got {bits:#x}, expected {first:#x}); the per-SCC sum order must be \
                 canonicalised by sorting loop IDs, not left to `HashMap` iteration",
            );
        }

        // Bit-parity with the full-pass helper pins that both paths
        // agree on the same canonical order (lex by loop id), not
        // just some determinism per path.
        let partitions_fwd = mapping(&seven_series.map(|(id, _)| (id, Some(0))));
        let full = compute_rel_loop_scores(&results, &partitions_fwd);
        let denom_fwd = compute_partition_denominator(&results, &partitions_fwd, Some(0));
        for id in [
            "aaa_01", "bbb_02", "ccc_03", "ddd_04", "eee_05", "fff_06", "ggg_07",
        ] {
            let streamed = compute_rel_loop_score_for_id(&results, id, &denom_fwd)
                .unwrap_or_else(|| panic!("loop '{id}' must have a computed series"));
            let cached = full.get(id).unwrap();
            assert_eq!(
                &streamed, cached,
                "streamed rel_loop_score for '{id}' must match full-pass bit-for-bit"
            );
        }
    }

    #[test]
    fn per_id_rel_loop_score_returns_none_for_unknown_loop() {
        // Matches `compute_rel_loop_scores` behaviour, which omits
        // loops whose `loop_score` is absent from `results.offsets`.
        let results = make_results_for_loops(&[("present", &[1.0, 2.0][..])]);
        let denom = vec![1.0, 2.0];
        assert!(compute_rel_loop_score_for_id(&results, "absent", &denom).is_none());
    }

    #[test]
    fn per_element_with_all_scalar_matches_scalar_variant() {
        // When every loop has slot count 1 (or absent from the map),
        // `compute_rel_loop_scores_per_element` must produce output
        // bit-identical to `compute_rel_loop_scores`.  Pins the
        // docstring claim that "scalar-only groups trivially stride 1".
        let series_a = &[1.0, 2.0, -4.0][..];
        let series_b = &[3.0, -4.0, 0.0][..];
        let results = make_results_for_loops(&[("A", series_a), ("B", series_b)]);
        let partitions = mapping(&[("A", Some(0)), ("B", Some(0))]);

        let scalar = compute_rel_loop_scores(&results, &partitions);

        // Empty slot map -> everything defaults to 1.
        let empty_slots: HashMap<String, usize> = HashMap::new();
        let per_elem_empty =
            compute_rel_loop_scores_per_element(&results, &partitions, &empty_slots);
        for (id, scalar_series) in &scalar {
            let per_elem_series = per_elem_empty
                .get(id)
                .unwrap_or_else(|| panic!("loop '{id}' missing from per-element output"));
            assert_eq!(per_elem_series, scalar_series);
        }

        // Explicit slot=1 -> identical result.
        let explicit_slots: HashMap<String, usize> = [("A".to_string(), 1), ("B".to_string(), 1)]
            .into_iter()
            .collect();
        let per_elem_explicit =
            compute_rel_loop_scores_per_element(&results, &partitions, &explicit_slots);
        assert_eq!(per_elem_explicit, scalar);
    }

    #[test]
    fn per_element_broadcasts_scalar_loops_in_mixed_partition() {
        // Partition containing one arrayed loop (3 slots) and one
        // scalar loop (1 slot): at element k, the arrayed loop
        // contributes slot k and the scalar loop broadcasts slot 0 to
        // every element's denominator.  This matches pre-PR compile-
        // time broadcast semantics and the test helper in
        // simulate_ltm.rs.
        //
        // Two steps, one partition:
        //   loop A (arrayed, 3 slots): step 0 -> [1, 2, 3], step 1 -> [2, 4, 6]
        //   loop B (scalar):           step 0 -> 4,         step 1 -> 8
        //
        // Denominators at each step should be:
        //   step 0, elem 0: |1| + |4| = 5
        //   step 0, elem 1: |2| + |4| = 6
        //   step 0, elem 2: |3| + |4| = 7
        //   step 1, elem 0: |2| + |8| = 10
        //   step 1, elem 1: |4| + |8| = 12
        //   step 1, elem 2: |6| + |8| = 14
        //
        // Results are stored row-major so we need 3 slots of A + 1
        // slot of B + the `time` column.  Build the Results manually
        // since `make_results_for_loops` assumes scalar loops.
        let step_count = 2;
        let step_size = 1 + 3 + 1; // time + 3 slots of A + 1 slot of B
        let mut data = vec![0.0_f64; step_count * step_size];
        // step 0
        data[0] = 0.0;
        data[1] = 1.0; // A[0]
        data[2] = 2.0; // A[1]
        data[3] = 3.0; // A[2]
        data[4] = 4.0; // B
        // step 1
        data[step_size] = 1.0;
        data[step_size + 1] = 2.0;
        data[step_size + 2] = 4.0;
        data[step_size + 3] = 6.0;
        data[step_size + 4] = 8.0;

        let mut offsets: HashMap<Ident<Canonical>, usize> = HashMap::new();
        offsets.insert(Ident::new("time"), 0);
        offsets.insert(loop_score_ident("A"), 1);
        offsets.insert(loop_score_ident("B"), 4);

        let sim_specs = SimSpecs {
            start: 0.0,
            stop: 1.0,
            dt: Dt::Dt(1.0),
            save_step: None,
            sim_method: SimMethod::Euler,
            time_units: None,
        };
        let results = Results {
            offsets,
            data: data.into_boxed_slice(),
            step_size,
            step_count,
            specs: Specs::from(&sim_specs),
            is_vensim: false,
        };

        let partitions = mapping(&[("A", Some(0)), ("B", Some(0))]);
        let slots: HashMap<String, usize> = [("A".to_string(), 3), ("B".to_string(), 1)]
            .into_iter()
            .collect();

        let per_elem = compute_rel_loop_scores_per_element(&results, &partitions, &slots);

        // Each loop's output Vec has length step_count * max_slots = 2 * 3 = 6,
        // with element k of step s at index s * 3 + k.
        let rel_a = per_elem.get("A").expect("loop A has a series");
        let rel_b = per_elem.get("B").expect("loop B has a series");
        assert_eq!(rel_a.len(), 6);
        assert_eq!(rel_b.len(), 6);

        // step 0
        assert!((rel_a[0] - (1.0 / 5.0)).abs() < 1e-12, "A[s=0, k=0]");
        assert!((rel_a[1] - (2.0 / 6.0)).abs() < 1e-12, "A[s=0, k=1]");
        assert!((rel_a[2] - (3.0 / 7.0)).abs() < 1e-12, "A[s=0, k=2]");
        assert!((rel_b[0] - (4.0 / 5.0)).abs() < 1e-12, "B[s=0, k=0]");
        assert!((rel_b[1] - (4.0 / 6.0)).abs() < 1e-12, "B[s=0, k=1]");
        assert!((rel_b[2] - (4.0 / 7.0)).abs() < 1e-12, "B[s=0, k=2]");

        // step 1
        assert!((rel_a[3] - (2.0 / 10.0)).abs() < 1e-12, "A[s=1, k=0]");
        assert!((rel_a[4] - (4.0 / 12.0)).abs() < 1e-12, "A[s=1, k=1]");
        assert!((rel_a[5] - (6.0 / 14.0)).abs() < 1e-12, "A[s=1, k=2]");
        assert!((rel_b[3] - (8.0 / 10.0)).abs() < 1e-12, "B[s=1, k=0]");
        assert!((rel_b[4] - (8.0 / 12.0)).abs() < 1e-12, "B[s=1, k=1]");
        assert!((rel_b[5] - (8.0 / 14.0)).abs() < 1e-12, "B[s=1, k=2]");
    }

    #[test]
    fn per_element_mixed_arity_drops_short_loop_beyond_its_length() {
        // A partition can contain arrayed loops with different slot
        // counts (e.g., a Region loop of size 2 sharing a partition
        // with a Region x Age loop of size 2*3 = 6 because they touch
        // the same stock SCC).  For elements `k >= slot_count_i`, the
        // shorter loop must drop out of both the denominator and the
        // numerator rather than reading past its own buffer into
        // adjacent columns.
        //
        // Test shape:
        //   loop A (2 slots): step 0 -> [1, 2]
        //   loop B (4 slots): step 0 -> [10, 20, 30, 40]
        //
        // max_slots = 4.  Expected denominators:
        //   k = 0: |1| + |10| = 11
        //   k = 1: |2| + |20| = 22
        //   k = 2:        |30| = 30   (A dropped)
        //   k = 3:        |40| = 40   (A dropped)
        //
        // A's output at k in {2, 3} must be the default 0.0 (loop has
        // no slot at that element, not a misaligned read from B's
        // buffer).
        let step_count = 1;
        let step_size = 1 + 2 + 4; // time + A + B
        let mut data = vec![0.0_f64; step_count * step_size];
        data[0] = 0.0;
        data[1] = 1.0; // A[0]
        data[2] = 2.0; // A[1]
        data[3] = 10.0; // B[0]
        data[4] = 20.0; // B[1]
        data[5] = 30.0; // B[2]
        data[6] = 40.0; // B[3]

        let mut offsets: HashMap<Ident<Canonical>, usize> = HashMap::new();
        offsets.insert(Ident::new("time"), 0);
        offsets.insert(loop_score_ident("A"), 1);
        offsets.insert(loop_score_ident("B"), 3);

        let sim_specs = SimSpecs {
            start: 0.0,
            stop: 0.0,
            dt: Dt::Dt(1.0),
            save_step: None,
            sim_method: SimMethod::Euler,
            time_units: None,
        };
        let results = Results {
            offsets,
            data: data.into_boxed_slice(),
            step_size,
            step_count,
            specs: Specs::from(&sim_specs),
            is_vensim: false,
        };

        let partitions = mapping(&[("A", Some(0)), ("B", Some(0))]);
        let slots: HashMap<String, usize> = [("A".to_string(), 2), ("B".to_string(), 4)]
            .into_iter()
            .collect();

        let per_elem = compute_rel_loop_scores_per_element(&results, &partitions, &slots);
        let rel_a = per_elem.get("A").expect("loop A has a series");
        let rel_b = per_elem.get("B").expect("loop B has a series");
        // Each Vec has length step_count * max_slots = 1 * 4 = 4.
        assert_eq!(rel_a.len(), 4);
        assert_eq!(rel_b.len(), 4);

        assert!((rel_a[0] - (1.0 / 11.0)).abs() < 1e-12, "A[k=0]");
        assert!((rel_a[1] - (2.0 / 22.0)).abs() < 1e-12, "A[k=1]");
        assert_eq!(rel_a[2], 0.0, "A[k=2] beyond A's length -> default 0.0");
        assert_eq!(rel_a[3], 0.0, "A[k=3] beyond A's length -> default 0.0");

        assert!((rel_b[0] - (10.0 / 11.0)).abs() < 1e-12, "B[k=0]");
        assert!((rel_b[1] - (20.0 / 22.0)).abs() < 1e-12, "B[k=1]");
        assert!((rel_b[2] - (30.0 / 30.0)).abs() < 1e-12, "B[k=2]");
        assert!((rel_b[3] - (40.0 / 40.0)).abs() < 1e-12, "B[k=3]");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        /// For any small random model, `compute_rel_loop_scores` must
        /// agree with the reference SAFEDIV formula to within 1e-10.
        /// Generators:
        ///   - 1..=6 loops, assigned to 1..=3 partitions.
        ///   - 1..=10 timesteps.
        ///   - loop_score samples in [-100, 100].
        #[test]
        fn matches_reference_formula(
            num_loops in 1usize..=6,
            num_partitions in 1usize..=3,
            num_steps in 1usize..=10,
            raw_values in prop::collection::vec(
                prop::collection::vec(-100.0_f64..=100.0_f64, 1..=10),
                1..=6,
            ),
            raw_partitions in prop::collection::vec(0usize..=2, 1..=6),
        ) {
            let num_loops = num_loops.min(raw_values.len()).min(raw_partitions.len());
            let num_steps = num_steps.min(raw_values[0].len());
            let num_partitions = num_partitions.max(1);

            // Build per-loop series with uniform step count.
            let series: Vec<Vec<f64>> = (0..num_loops)
                .map(|i| raw_values[i].iter().copied().take(num_steps).collect())
                .collect();
            for s in &series {
                prop_assume!(s.len() == num_steps);
            }

            let loop_ids: Vec<String> = (0..num_loops).map(|i| format!("L{i}")).collect();
            let loop_partitions: HashMap<String, Option<usize>> = loop_ids
                .iter()
                .enumerate()
                .map(|(i, id)| (id.clone(), Some(raw_partitions[i] % num_partitions)))
                .collect();

            // Build Results matching the series.
            let pair_refs: Vec<(&str, &[f64])> = loop_ids
                .iter()
                .zip(series.iter())
                .map(|(id, s)| (id.as_str(), s.as_slice()))
                .collect();
            let results = make_results_for_loops(&pair_refs);

            let scored = compute_rel_loop_scores(&results, &loop_partitions);
            let expected = reference_rel_loop_scores(&loop_ids, &loop_partitions, &series);

            for (i, id) in loop_ids.iter().enumerate() {
                let actual_series = scored.get(id).expect("every loop has a series");
                prop_assert_eq!(actual_series.len(), num_steps);
                for t in 0..num_steps {
                    let a = actual_series[t];
                    let e = expected[i][t];
                    // Both NaN counts as a match (shouldn't occur given
                    // the finite generator range, but safeguard anyway).
                    if a.is_nan() && e.is_nan() {
                        continue;
                    }
                    prop_assert!(
                        (a - e).abs() <= 1e-10,
                        "loop {} t={}: actual={} expected={}", id, t, a, e
                    );
                }
            }
        }
    }
}
