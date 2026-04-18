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

use std::collections::HashMap;

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
