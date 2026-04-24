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
///
/// ## Arrayed (A2A) loops read slot 0 only
///
/// For arrayed loops whose `loop_score` variable occupies multiple
/// slots in `results`, this function reads only the first slot
/// (element 0) for both the numerator and the partition denominator.
/// That matches the pre-PR FFI semantics (which also returned a
/// scalar series per loop), so existing libsimlin/pysimlin/TS
/// callers see no behaviour change.  Callers that need genuine
/// per-element normalization -- e.g. a dimension-aware importance
/// ranking in the diagram UI, or a future FFI that exposes arrayed
/// loop analysis -- should use
/// [`compute_rel_loop_scores_per_element`], which reproduces the
/// pre-PR compile-time per-element math.
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

    let mut partition_groups: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
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

/// Per-timestep, per-element relative loop scores for arrayed (A2A)
/// loops.
///
/// [`compute_rel_loop_scores`] collapses every loop's `loop_score` to
/// slot 0.  That matches the scalar FFI contract, but pre-PR's
/// compile-time `rel_loop_score` synthetic variables were genuinely
/// per-element for A2A loops; callers that want the same dimension-
/// aware view (diagram UI per-element importance, dimension-aware
/// pysimlin consumers, a future arrayed FFI) need a path that
/// reproduces that math from post-sim `loop_score` data.
///
/// Returns a flat `Vec<f64>` per loop id of length
/// `step_count * max_slots`, where `max_slots` is the largest slot
/// count among the loops sharing the loop's partition group.  The
/// value at step `s`, element `k` is at index `s * max_slots + k`.
/// Scalar loops in a mixed partition broadcast their single value
/// across every element slot, which is what the pre-PR compile-time
/// emitter did (a scalar loop_score referenced from an A2A
/// rel_loop_score equation expanded uniformly across the target's
/// elements).
///
/// `n_slots_by_loop` maps each loop id to its element count.  Missing
/// entries or a count of 1 are treated as scalar.  The denominator at
/// element `k` is `Σ_j |loop_score_j[k_j]|` where `k_j = k` for
/// arrayed loops and `k_j = 0` for scalar ones.  SAFEDIV-0 and NaN
/// propagation match [`compute_rel_loop_scores`].
///
/// `BTreeMap` on partition groups keeps the float summation order
/// deterministic across runs, the same rationale
/// [`compute_rel_loop_scores`] documents for its own grouping.
pub fn compute_rel_loop_scores_per_element(
    results: &Results,
    loop_partitions: &HashMap<String, Option<usize>>,
    n_slots_by_loop: &HashMap<String, usize>,
) -> HashMap<String, Vec<f64>> {
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

    // Per-group max_slots is the stride used for both the numerator
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
                let denom: f64 = indices
                    .iter()
                    .filter_map(|&i| {
                        offsets[i].map(|off| {
                            let elem = if slot_counts[i] > 1 { k } else { 0 };
                            row[off + elem].abs()
                        })
                    })
                    .sum();
                for &i in indices {
                    let Some(off) = offsets[i] else { continue };
                    let elem = if slot_counts[i] > 1 { k } else { 0 };
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

/// Compute the cycle-partition denominator series:
/// `denominator[t] = Σ_{j in partition} |loop_score[j, t]|`.
///
/// Loops in `loop_ids` whose `loop_score` variable is absent from
/// `results` (e.g. LTM disabled for that loop, discovery-mode
/// compilation, or model truncation) are omitted from the sum --
/// the same semantics [`compute_rel_loop_scores`] uses.  Returns a
/// length-`results.step_count` `Vec`, zero-filled when the
/// partition is empty.
///
/// Exposed separately from [`compute_rel_loop_scores`] so that
/// FFI callers that query one loop at a time (e.g.
/// `simlin_analyze_get_relative_loop_score` iterated over a
/// project's loops) can cache the per-partition denominator on
/// the sim state and avoid recomputing it on every call.  Paired
/// with [`compute_rel_loop_score_for_id`].
///
/// Element-0 scalar semantics: for arrayed loops whose
/// `loop_score` variable occupies multiple slots, this reads only
/// the first slot.  See [`compute_rel_loop_scores`] for the
/// pre-PR-FFI rationale, and
/// [`compute_rel_loop_scores_per_element`] for a dimension-aware
/// alternative.
pub fn compute_partition_denominator<'a, I>(results: &Results, loop_ids: I) -> Vec<f64>
where
    I: IntoIterator<Item = &'a str>,
{
    let offsets: Vec<usize> = loop_ids
        .into_iter()
        .filter_map(|id| results.offsets.get(&loop_score_ident(id)).copied())
        .collect();

    let mut denom = vec![0.0_f64; results.step_count];
    for (t, row) in results.iter().enumerate() {
        denom[t] = offsets.iter().map(|&off| row[off].abs()).sum();
    }
    denom
}

/// Compute a single loop's relative-loop-score series, given a
/// pre-computed partition denominator from
/// [`compute_partition_denominator`].
///
/// Returns `None` when the loop's `loop_score` variable is absent
/// from `results` (matching [`compute_rel_loop_scores`], which
/// simply omits those loops from its output map).  SAFEDIV-0
/// semantics: `denominator[t] == 0` yields `0`, not `NaN`.
/// Non-finite numerators propagate through normal IEEE-754
/// arithmetic, matching the behaviour of the retired compile-time
/// emitter.
///
/// The caller is responsible for ensuring `denominator` covers the
/// same partition the loop belongs to, and that its length matches
/// `results.step_count`.
///
/// Element-0 scalar semantics: for arrayed loops whose
/// `loop_score` variable occupies multiple slots, this reads only
/// the first slot.  See [`compute_rel_loop_scores_per_element`]
/// for dimension-aware output.
pub fn compute_rel_loop_score_for_id(
    results: &Results,
    loop_id: &str,
    denominator: &[f64],
) -> Option<Vec<f64>> {
    let off = results.offsets.get(&loop_score_ident(loop_id)).copied()?;
    let mut out = Vec::with_capacity(results.step_count);
    for (t, row) in results.iter().enumerate() {
        let num = row[off];
        let denom = denominator[t];
        out.push(if denom == 0.0 { 0.0 } else { num / denom });
    }
    Some(out)
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
        let mut groups: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
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

    /// The streaming `compute_partition_denominator` +
    /// `compute_rel_loop_score_for_id` pair must produce the same
    /// per-loop series as the full-sweep `compute_rel_loop_scores`
    /// -- that is the contract the libsimlin FFI cache relies on.
    #[test]
    fn per_id_helpers_match_full_sweep() {
        let series_a = &[1.0, 2.0, -4.0, 0.0][..];
        let series_b = &[3.0, -4.0, 0.0, 7.0][..];
        let series_c = &[0.5, 0.5, 0.5, 0.5][..];
        let results = make_results_for_loops(&[("A", series_a), ("B", series_b), ("C", series_c)]);
        let partitions = mapping(&[("A", Some(0)), ("B", Some(0)), ("C", Some(1))]);

        let full = compute_rel_loop_scores(&results, &partitions);

        // Partition 0 contains A and B.
        let denom_0 = compute_partition_denominator(&results, ["A", "B"]);
        let rel_a = compute_rel_loop_score_for_id(&results, "A", &denom_0).unwrap();
        let rel_b = compute_rel_loop_score_for_id(&results, "B", &denom_0).unwrap();

        // Partition 1 contains only C.
        let denom_1 = compute_partition_denominator(&results, ["C"]);
        let rel_c = compute_rel_loop_score_for_id(&results, "C", &denom_1).unwrap();

        for (id, streamed) in [("A", &rel_a), ("B", &rel_b), ("C", &rel_c)] {
            let expected = full.get(id).expect("full-sweep must have this loop");
            assert_eq!(
                streamed.len(),
                expected.len(),
                "series length mismatch for {id}"
            );
            for t in 0..expected.len() {
                // Bit-for-bit: the two paths multiply and divide the
                // same floats in the same order, so rounding must match.
                assert_eq!(
                    streamed[t], expected[t],
                    "loop {id} t={t}: streamed {} vs full {}",
                    streamed[t], expected[t]
                );
            }
        }
    }

    /// A loop whose `loop_score` variable is absent must return
    /// `None`, matching the "omit absent loops" contract of the
    /// full-sweep API.
    #[test]
    fn per_id_helper_returns_none_for_absent_loop() {
        let results = make_results_for_loops(&[("A", &[1.0, 2.0][..])]);
        let denom = compute_partition_denominator(&results, ["A"]);
        assert!(compute_rel_loop_score_for_id(&results, "missing", &denom).is_none());
    }

    /// Per-element variant: two A2A loops in a shared partition, each
    /// with 3 element slots.  At every element k, the sum of absolute
    /// rel-scores across the partition must equal 1.0 (non-zero
    /// elements) or 0.0 (zero-denominator elements) independently --
    /// that is the whole reason the per-element helper exists.  The
    /// scalar path collapses to slot 0, which would sum to 1.0 only
    /// for element 0 and miss the others.
    #[test]
    fn per_element_helper_normalizes_within_each_slot() {
        let n_slots: usize = 3;
        let step_count: usize = 4;
        // Two A2A loops with distinct per-element magnitudes so each
        // element has a meaningful partition split.
        //   A: [1, 3,  5, 2, ...] per element 0, 1, 2, ...
        //   B: [3, 1, 15, 6, ...] per element 0, 1, 2, ...
        // Constructing by steps * elements and writing directly into
        // a Results layout avoids coupling to the rest of the engine.
        let mut data = vec![0.0_f64; step_count * (2 * n_slots + 1)];
        let step_size = 2 * n_slots + 1;
        let a_off = 1;
        let b_off = 1 + n_slots;
        for step in 0..step_count {
            let row = &mut data[step * step_size..(step + 1) * step_size];
            row[0] = step as f64; // time
            for k in 0..n_slots {
                row[a_off + k] = ((step + 1) * (k + 1)) as f64;
                row[b_off + k] = ((step + 1) * (k + 2)) as f64;
            }
        }
        let mut offsets: HashMap<Ident<Canonical>, usize> = HashMap::new();
        offsets.insert(Ident::new("time"), 0);
        offsets.insert(loop_score_ident("A"), a_off);
        offsets.insert(loop_score_ident("B"), b_off);

        let sim_specs = crate::datamodel::SimSpecs {
            start: 0.0,
            stop: (step_count - 1) as f64,
            dt: crate::datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: crate::datamodel::SimMethod::Euler,
            time_units: None,
        };
        let results = Results {
            offsets,
            data: data.into_boxed_slice(),
            step_size,
            step_count,
            specs: crate::results::Specs::from(&sim_specs),
            is_vensim: false,
        };

        let partitions = mapping(&[("A", Some(0)), ("B", Some(0))]);
        let mut slots = HashMap::new();
        slots.insert("A".to_string(), n_slots);
        slots.insert("B".to_string(), n_slots);

        let rel = compute_rel_loop_scores_per_element(&results, &partitions, &slots);
        let a = rel.get("A").expect("A must have a series");
        let b = rel.get("B").expect("B must have a series");
        assert_eq!(a.len(), step_count * n_slots);
        assert_eq!(b.len(), step_count * n_slots);

        for step in 0..step_count {
            for k in 0..n_slots {
                let idx = step * n_slots + k;
                let sum = a[idx].abs() + b[idx].abs();
                // Magnitudes per element are finite and non-zero here,
                // so the sum of absolute rel-scores must be 1.0 with
                // full float precision.
                assert!(
                    (sum - 1.0).abs() < 1e-12,
                    "step {step} elem {k}: |a|+|b| = {sum}, not 1.0"
                );
            }
        }
    }

    /// Mixed partition: one scalar loop and one A2A loop.  The scalar
    /// loop's single slot broadcasts into every element of the
    /// partition's max-slots denominator -- this matches the pre-PR
    /// compile-time emitter, which expanded a scalar loop_score
    /// reference across the arrayed rel_loop_score target.
    #[test]
    fn per_element_helper_broadcasts_scalar_across_elements() {
        let n_slots: usize = 2;
        let step_count: usize = 2;
        // Layout: time | A (scalar, 1 slot) | B (A2A, 2 slots)
        let step_size = 1 + 1 + n_slots;
        let a_off = 1;
        let b_off = 2;
        let mut data = vec![0.0_f64; step_count * step_size];
        // A[t=0] = 2, B[t=0] = [3, 6];   denominators = [5, 8]
        // A[t=1] = 1, B[t=1] = [1, 4];   denominators = [2, 5]
        let a_vals = [2.0_f64, 1.0];
        let b_vals = [[3.0_f64, 6.0], [1.0, 4.0]];
        for step in 0..step_count {
            let row = &mut data[step * step_size..(step + 1) * step_size];
            row[0] = step as f64;
            row[a_off] = a_vals[step];
            row[b_off..b_off + n_slots].copy_from_slice(&b_vals[step][..n_slots]);
        }
        let mut offsets: HashMap<Ident<Canonical>, usize> = HashMap::new();
        offsets.insert(Ident::new("time"), 0);
        offsets.insert(loop_score_ident("A"), a_off);
        offsets.insert(loop_score_ident("B"), b_off);

        let sim_specs = crate::datamodel::SimSpecs {
            start: 0.0,
            stop: (step_count - 1) as f64,
            dt: crate::datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: crate::datamodel::SimMethod::Euler,
            time_units: None,
        };
        let results = Results {
            offsets,
            data: data.into_boxed_slice(),
            step_size,
            step_count,
            specs: crate::results::Specs::from(&sim_specs),
            is_vensim: false,
        };

        let partitions = mapping(&[("A", Some(0)), ("B", Some(0))]);
        let mut slots = HashMap::new();
        slots.insert("A".to_string(), 1); // scalar
        slots.insert("B".to_string(), n_slots);

        let rel = compute_rel_loop_scores_per_element(&results, &partitions, &slots);
        let a = rel.get("A").unwrap();
        let b = rel.get("B").unwrap();
        assert_eq!(a.len(), step_count * n_slots);
        assert_eq!(b.len(), step_count * n_slots);

        let at = |step: usize, k: usize| step * n_slots + k;

        // Element 0: denom t0 = |2| + |3| = 5; denom t1 = |1| + |1| = 2.
        assert!((a[at(0, 0)] - (2.0 / 5.0)).abs() < 1e-12);
        assert!((b[at(0, 0)] - (3.0 / 5.0)).abs() < 1e-12);
        assert!((a[at(1, 0)] - (1.0 / 2.0)).abs() < 1e-12);
        assert!((b[at(1, 0)] - (1.0 / 2.0)).abs() < 1e-12);

        // Element 1: scalar A broadcasts its slot-0 value.  denom t0 =
        // |2| + |6| = 8; denom t1 = |1| + |4| = 5.  This is the
        // property that the scalar-only helpers cannot express.
        assert!((a[at(0, 1)] - (2.0 / 8.0)).abs() < 1e-12);
        assert!((b[at(0, 1)] - (6.0 / 8.0)).abs() < 1e-12);
        assert!((a[at(1, 1)] - (1.0 / 5.0)).abs() < 1e-12);
        assert!((b[at(1, 1)] - (4.0 / 5.0)).abs() < 1e-12);
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
