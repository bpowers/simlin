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

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::common::{Canonical, Ident};
use crate::results::Results;

/// A `(partition, slot)` bucket key for the per-element normalization grid.
type BucketKey = (Option<usize>, usize);
/// A `(loop_index, read_slot)` pair: which loop contributes to a bucket and
/// which of its own `loop_score` slots is read (0 for a broadcast scalar
/// loop, the bucket's slot for an arrayed loop).
type BucketMember = (usize, usize);

/// One loop's `|loop_score|` contribution to a partition-sum denominator,
/// with `NaN` summands excluded.
///
/// A `NaN` loop_score at a step means that one loop's score is *undefined*
/// there; it is not signal, so it must not flow into the partition sum --
/// otherwise a single bad loop turns the whole partition's denominator into
/// `NaN` (the `denom == 0.0` SAFEDIV guard does not fire on `NaN`, since
/// `NaN == 0.0` is false), poisoning every sibling's relative score (GH
/// #542).  Dropping the `NaN` summand lets healthy siblings normalize
/// against the healthy denominator; the bad loop's *own* numerator stays
/// `NaN`, so its own relative score stays `NaN` -- the honest per-loop
/// "undefined here" signal.  This matches the discovery path, which coerces
/// a `NaN` link score to a non-contributing 0
/// (`ltm_finding::SearchGraph::from_edges`).
///
/// `+/-Inf` is deliberately NOT excluded: a raw loop score legitimately
/// diverges at a dominance inflection (the link-score denominators go to
/// zero there), so an `Inf` summand is real signal that the loop dominates.
/// Keeping it in the sum sends the dominated siblings to `0` (`finite/Inf`)
/// and the dominant loop to `NaN` (`Inf/Inf`) -- the same inflection-point
/// behaviour the removed SAFEDIV equation produced, preserved bug-for-bug.
#[inline]
fn denom_summand(v: f64) -> f64 {
    if v.is_nan() { 0.0 } else { v.abs() }
}

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

/// Slot count of a loop's `loop_score` series from its per-slot partition
/// vector: 1 for a scalar / cross-element / mixed loop, the dimension
/// element-space size for an A2A loop.  Mirrors
/// `ltm_post::build_loop_element_index`'s `n_slots`, since both are derived
/// from the same `LtmSyntheticVar` metadata in `model_ltm_variables`.
fn loop_n_slots(loop_partitions: &HashMap<String, Vec<Option<usize>>>, id: &str) -> usize {
    loop_partitions.get(id).map(|v| v.len()).unwrap_or(1).max(1)
}

/// The partition (`Option<usize>`) of loop `id` at slot `k`.
///
/// For an arrayed loop this is `loop_partitions[id][k]`; for a scalar loop
/// (`n_slots == 1`) it is `loop_partitions[id][0]` broadcast across every
/// `k` -- a scalar loop has no elements, so it carries its single partition
/// into every slot it is compared in (the same broadcast the pre-PR
/// compile-time emitter applied when a scalar `loop_score` was referenced
/// from an arrayed `rel_loop_score` equation).  `None` (out-of-range `k` on
/// an arrayed loop, or a genuinely-`None` partition) means "no contribution
/// at this slot for the purpose of *that loop's own* series", though it
/// still buckets into the `None` cohort.
fn slot_partition(
    loop_partitions: &HashMap<String, Vec<Option<usize>>>,
    id: &str,
    k: usize,
) -> Option<usize> {
    let v = loop_partitions.get(id)?;
    if v.len() <= 1 {
        v.first().copied().flatten()
    } else {
        v.get(k).copied().flatten()
    }
}

/// Compute per-loop, per-timestep relative loop scores from simulated
/// `loop_score` data -- the **slot-0 convenience view**.
///
/// For each loop whose `loop_score` series is present in `results`, the
/// returned value is:
///
/// ```text
/// rel_loop_score[i, t] = loop_score[i, t, 0] / Σ_{j : slot-0 partition of j == slot-0 partition of i} |loop_score[j, t, 0]|
/// ```
///
/// `loop_partitions` maps each loop ID to its **per-slot** cycle-partition
/// vector (as produced by `model_ltm_variables`; length 1 for a
/// scalar/cross-element/mixed loop, one entry per element for an A2A loop).
/// This function reports only slot 0 for every loop and groups loops by
/// their *slot-0* partition (`loop_partitions[id][0]`) -- the catch-all
/// `None` cohort still groups together for loops genuinely below the parent
/// graph.  This preserves the pre-Phase-2 scalar contract (one series per
/// loop), so existing libsimlin/pysimlin/TS callers see no shape change;
/// callers that want genuine per-element normalization use
/// [`compute_rel_loop_scores_per_element`].
///
/// The denominator uses SAFEDIV-0 semantics: when the partition sum at
/// slot 0 is `0` the result is `0` rather than `NaN`.
///
/// Non-finite `loop_score` handling (GH #542): a `NaN` summand is
/// *excluded* from the partition sum via [`denom_summand`], so one loop
/// whose score is undefined at a step no longer poisons every sibling's
/// relative score in that partition.  The bad loop's own numerator is
/// still `NaN`, so its own relative score stays `NaN` -- the honest
/// per-loop "undefined here" signal -- while healthy siblings normalize
/// against the healthy denominator.  This matches the discovery path's
/// "a `NaN` link contributes nothing" rule
/// (`ltm_finding::SearchGraph::from_edges`).  `+/-Inf` is deliberately
/// *kept* in the sum: a raw loop score legitimately diverges at a
/// dominance inflection, so `Inf` is real signal -- it sends the
/// dominated siblings to `0` and the dominant loop to `NaN` (`Inf/Inf`),
/// the same inflection-point behaviour the removed SAFEDIV equation
/// produced.  Earlier this whole module propagated `NaN` through normal
/// IEEE-754 arithmetic only because the post-simulation refactor
/// preserved the removed synthetic equation's semantics bug-for-bug; the
/// removed SAFEDIV never promised `NaN`-resilience either.
///
/// Loops whose `loop_score` is absent from `results` (e.g. LTM disabled
/// for that loop, or discovery-mode compilation) are omitted from the
/// returned map.
///
/// # Lone-pin degeneracy
///
/// A modeler-pinned loop (`pin{n}` id) is registered in its own single-slot
/// `loop_partitions` entry.  When a pin is the *only* loop in its partition --
/// always the case in discovery mode (no enumerated loop scores exist there)
/// and in exhaustive mode whenever the pin is the lone loop through its stock
/// -- the partition sum equals `|loop_score[pin]|`, so the relative score
/// collapses to exactly `+1` (or `-1`, carrying the raw score's sign) whenever
/// the loop is active and `0` (via SAFEDIV-0) when its raw score is `0`.  This
/// is intentional: a fraction-of-all-known-loops normalization is undefined
/// for a partition of one.  Callers that want a pinned loop's actual magnitude
/// should read its **raw** `loop_score` series directly.  Two or more pins (or
/// a pin plus enumerated loops) on stocks in the *same* SCC partition normalize
/// against each other normally.
pub fn compute_rel_loop_scores(
    results: &Results,
    loop_partitions: &HashMap<String, Vec<Option<usize>>>,
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

    // Group loops by their slot-0 partition (the convenience-view key).
    let mut partition_groups: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    for (i, id) in loop_ids.iter().enumerate() {
        partition_groups
            .entry(slot_partition(loop_partitions, id, 0))
            .or_default()
            .push(i);
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
                .filter_map(|&i| offsets[i].map(|off| denom_summand(row[off])))
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

/// Per-timestep, per-slot relative loop scores, grouped by
/// `(partition, slot)`.
///
/// [`compute_rel_loop_scores`] collapses every loop's `loop_score` to
/// slot 0.  This function keeps every slot, and -- crucially -- groups
/// slots by `(slot_partition(id, k), k)` rather than by a single per-loop
/// partition.  So an A2A loop over an element-wise-coupled dimension (every
/// slot in partition `p`) lands in buckets `(p, 0)`, `(p, 1)`, ...; an A2A
/// loop over an element-wise-uncoupled dimension spreads across `(p0, 0)`,
/// `(p1, 1)`, ... -- which is precisely why two disconnected per-element
/// feedback subsystems over the same dimension stop cross-normalizing
/// (GH #487).
///
/// `loop_partitions` is the per-slot partition map from
/// `model_ltm_variables`; the loop's slot count is `loop_partitions[id].len()`
/// (no separate slot-count map is threaded).  Returns a flat `Vec<f64>` per
/// loop id; the value at step `s`, slot `k` is at index `s * stride + k`,
/// where `stride` is the loop's own slot count for an arrayed loop, and for
/// a scalar loop the largest slot index its (slot-0) partition covers + 1
/// (1 if no arrayed loop shares that partition).  A scalar loop broadcasts
/// its single value into every slot of its partition's buckets -- the same
/// broadcast the pre-PR compile-time emitter applied when a scalar
/// `loop_score` was referenced from an arrayed `rel_loop_score` equation.
///
/// Denominator at bucket `(p, k)` at step `s` is `Σ |loop_score[j, s, rs_j]|`
/// over the members of that bucket, where `rs_j` is `0` for a scalar member
/// (broadcast) and `k` for an arrayed member with `k < n_slots[j]` (an
/// arrayed loop with `k >= n_slots[j]` is not a member of slot-`k` buckets).
/// SAFEDIV-0 semantics, per-bucket `NaN` exclusion, and `Inf` retention
/// all match [`compute_rel_loop_scores`] (via [`denom_summand`]): a `NaN`
/// at one `(loop, slot)` does not poison the rest of its `(partition,
/// slot)` bucket (GH #542), while an `Inf` at one slot stays in that
/// bucket's denominator.  `BTreeMap` on the bucket grid keeps the float
/// summation order deterministic across runs.
pub fn compute_rel_loop_scores_per_element(
    results: &Results,
    loop_partitions: &HashMap<String, Vec<Option<usize>>>,
) -> HashMap<String, Vec<f64>> {
    let mut loop_ids: Vec<&String> = loop_partitions.keys().collect();
    loop_ids.sort();

    let offsets: Vec<Option<usize>> = loop_ids
        .iter()
        .map(|id| results.offsets.get(&loop_score_ident(id)).copied())
        .collect();
    let n_slots: Vec<usize> = loop_ids
        .iter()
        .map(|id| loop_n_slots(loop_partitions, id))
        .collect();

    // For each partition, the set of slot indices where some loop is in it.
    // A scalar loop contributes its single slot 0; an arrayed loop
    // contributes its per-slot partitions.  This drives the broadcast stride
    // for scalar loops (a scalar loop in partition `p` is "compared in" every
    // slot of `p`'s buckets) and lets us pre-build the bucket membership.
    let mut partition_slots: BTreeMap<Option<usize>, BTreeSet<usize>> = BTreeMap::new();
    for (i, id) in loop_ids.iter().enumerate() {
        for k in 0..n_slots[i] {
            partition_slots
                .entry(slot_partition(loop_partitions, id, k))
                .or_default()
                .insert(k);
        }
    }

    // Per-loop output stride: an arrayed loop's own slot count; a scalar
    // loop's (slot-0) partition's largest covered slot index + 1.
    let strides: Vec<usize> = loop_ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            if n_slots[i] > 1 {
                n_slots[i]
            } else {
                let p = slot_partition(loop_partitions, id, 0);
                partition_slots
                    .get(&p)
                    .and_then(|ks| ks.iter().max().copied())
                    .map(|m| m + 1)
                    .unwrap_or(1)
                    .max(1)
            }
        })
        .collect();

    // Pre-build the `(partition, slot)` -> [(loop_idx, read_slot)] grid.
    // `read_slot` is 0 for a scalar member (broadcast) and `k` for an arrayed
    // member; arrayed members past their own `n_slots` are not in any
    // slot-`k` bucket (no OOB read past their own `loop_score` slots).
    let mut members: BTreeMap<BucketKey, Vec<BucketMember>> = BTreeMap::new();
    for (i, id) in loop_ids.iter().enumerate() {
        if offsets[i].is_none() {
            continue;
        }
        if n_slots[i] <= 1 {
            // Scalar loop: appears in every slot of its (slot-0) partition,
            // always reading slot 0.
            let p = slot_partition(loop_partitions, id, 0);
            if let Some(ks) = partition_slots.get(&p) {
                for &k in ks {
                    members.entry((p, k)).or_default().push((i, 0));
                }
            }
        } else {
            for k in 0..n_slots[i] {
                let p = slot_partition(loop_partitions, id, k);
                members.entry((p, k)).or_default().push((i, k));
            }
        }
    }

    let mut series: Vec<Vec<f64>> = offsets
        .iter()
        .enumerate()
        .map(|(i, o)| {
            if o.is_some() {
                vec![0.0_f64; results.step_count * strides[i]]
            } else {
                Vec::new()
            }
        })
        .collect();

    for (step, row) in results.iter().enumerate() {
        for (&(_p, k), member_list) in &members {
            let denom: f64 = member_list
                .iter()
                .filter_map(|&(i, rs)| offsets[i].map(|off| denom_summand(row[off + rs])))
                .sum();
            for &(i, rs) in member_list {
                let Some(off) = offsets[i] else { continue };
                let num = row[off + rs];
                let val = if denom == 0.0 { 0.0 } else { num / denom };
                series[i][step * strides[i] + k] = val;
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
/// `denominator[t] = Σ_{j in partition, loop_score[j, t] not NaN} |loop_score[j, t]|`.
///
/// Loops in `loop_ids` whose `loop_score` variable is absent from
/// `results` (e.g. LTM disabled for that loop, discovery-mode
/// compilation, or model truncation) are omitted from the sum --
/// the same semantics [`compute_rel_loop_scores`] uses.  `NaN`
/// summands are excluded and `Inf` retained (via [`denom_summand`],
/// GH #542), so this streaming denominator stays bit-for-bit
/// identical to the full-sweep [`compute_rel_loop_scores`] sum.
/// Returns a length-`results.step_count` `Vec`, zero-filled when the
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
        denom[t] = offsets.iter().map(|&off| denom_summand(row[off])).sum();
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
/// semantics: `denominator[t] == 0` yields `0`, not `NaN`.  This
/// loop's *own* numerator propagates through normal IEEE-754
/// arithmetic: a `NaN` numerator yields a `NaN` relative score
/// (the honest per-loop "undefined here" signal), and an `Inf`
/// numerator over a finite denom yields `Inf`.  The cross-loop
/// `NaN`-poisoning fix lives in the denominator
/// ([`compute_partition_denominator`] excludes `NaN` summands, GH
/// #542), not here.
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

/// Resolve the slot offset to read for a loop with `n_slots` slots when
/// the partition is being queried at `element_index`.
///
/// - Scalar loops (`n_slots <= 1`) → `Some(0)` -- slot 0 broadcasts
///   across every partition element.
/// - Arrayed loops with `element_index < n_slots` → `Some(element_index)`.
/// - Arrayed loops with `element_index >= n_slots` → `None` -- the loop
///   has no own element at this partition index and must not contribute
///   (matches the gating that [`compute_rel_loop_scores_per_element`]
///   applies in the full-sweep path).
///
/// Returning `None` rather than clamping to `n_slots - 1` matters for
/// mixed-stride partitions where two arrayed loops have different
/// dimensionalities: the loop that runs out of slots first does NOT
/// stand in for the larger loop's later elements.  Callers (the
/// streaming partition denominator and per-loop helpers) skip
/// `None`-returning members so the FFI's amortised path stays
/// bit-for-bit consistent with the full-sweep helper.
fn effective_slot(n_slots: usize, element_index: usize) -> Option<usize> {
    if n_slots <= 1 {
        Some(0)
    } else if element_index < n_slots {
        Some(element_index)
    } else {
        None
    }
}

/// Per-element streaming variant of [`compute_partition_denominator`].
///
/// For each `(loop_id, n_slots)` in the iterator whose `loop_score`
/// variable is present in `results`, contributes
/// `|row[off + slot]|` to the partition sum at every step, where
/// `slot` is determined by [`effective_slot`]:
///   - Scalar loops (`n_slots <= 1`) contribute slot 0 (broadcast).
///   - Arrayed loops with `element_index < n_slots` contribute their
///     own slot at `element_index`.
///   - Arrayed loops with `element_index >= n_slots` do NOT contribute
///     -- the loop has no own element at this partition index.
///
/// A contributing slot's value goes through [`denom_summand`], so a
/// `NaN` slot is excluded from the sum and an `Inf` slot retained (GH
/// #542) -- the same per-bucket `NaN`-isolation the full-sweep
/// [`compute_rel_loop_scores_per_element`] applies.
///
/// This skip-vs-clamp distinction matters for mixed-stride partitions
/// (two arrayed loops with different dimensionalities sharing a
/// partition).  Producing the same partition sums as the full-sweep
/// [`compute_rel_loop_scores_per_element`] is the contract the
/// libsimlin FFI per-partition cache relies on; the streaming pair
/// must be a strictly cheaper path to the same numbers, not an
/// approximation.
///
/// Exposed alongside [`compute_partition_denominator`] so the libsimlin
/// FFI per-partition cache can amortize across element-aware queries
/// (cache key `(partition, element_index)`) without falling back to the
/// non-streaming [`compute_rel_loop_scores_per_element`].
pub fn compute_partition_denominator_for_element<'a, I>(
    results: &Results,
    loop_id_slots: I,
    element_index: usize,
) -> Vec<f64>
where
    I: IntoIterator<Item = (&'a str, usize)>,
{
    let entries: Vec<(usize, usize)> = loop_id_slots
        .into_iter()
        .filter_map(|(id, n_slots)| {
            let off = results.offsets.get(&loop_score_ident(id)).copied()?;
            let slot = effective_slot(n_slots, element_index)?;
            Some((off, slot))
        })
        .collect();

    let mut denom = vec![0.0_f64; results.step_count];
    for (t, row) in results.iter().enumerate() {
        denom[t] = entries
            .iter()
            .map(|&(off, slot)| denom_summand(row[off + slot]))
            .sum();
    }
    denom
}

/// Per-element streaming variant of [`compute_rel_loop_score_for_id`].
///
/// Reads `row[off + slot]` as the numerator at each step, where `slot`
/// is determined by [`effective_slot`]:
///   - Scalar loops (`n_slots <= 1`) read slot 0 (broadcast).
///   - Arrayed loops with `element_index < n_slots` read their own slot.
///   - Arrayed loops with `element_index >= n_slots` return all zeros
///     -- the loop has no own element at this partition index, matching
///     the zero-fill that [`compute_rel_loop_scores_per_element`]
///     applies in the full-sweep path.
///
/// Paired with [`compute_partition_denominator_for_element`] for SAFEDIV
/// normalisation.  Returns `None` only when the loop's `loop_score`
/// variable is entirely absent from `results` (matching the scalar
/// streaming helper's "absent loop" contract); a present-but-no-element
/// query yields all-zeros, not `None`.
pub fn compute_rel_loop_score_for_element(
    results: &Results,
    loop_id: &str,
    n_slots: usize,
    element_index: usize,
    denominator: &[f64],
) -> Option<Vec<f64>> {
    let off = results.offsets.get(&loop_score_ident(loop_id)).copied()?;
    let Some(slot) = effective_slot(n_slots, element_index) else {
        // This loop has no own element at the queried partition index.
        // Return zero-fill rather than reading another loop's slot.
        return Some(vec![0.0; results.step_count]);
    };
    let mut out = Vec::with_capacity(results.step_count);
    for (t, row) in results.iter().enumerate() {
        let num = row[off + slot];
        let denom = denominator[t];
        out.push(if denom == 0.0 { 0.0 } else { num / denom });
    }
    Some(out)
}

/// Per-loop dimension metadata used by the FFI subscript resolver to
/// turn `r1[Boston]` (or `r1[Boston, 2]`) into a concrete slot offset.
///
/// All names are stored in canonical (lowercased, separator-normalized)
/// form so the resolver can compare them directly against canonicalized
/// user input -- no per-call canonicalize allocations.  Indexed
/// dimensions store an empty `dim_elements` entry; the resolver parses
/// their subscripts as 1-based integers and validates against
/// `dim_sizes` instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopElementIndex {
    /// Canonical dimension names in declaration order (matches the
    /// equation-language subscript order: `r1[d0, d1, ...]`).
    pub dimensions: Vec<String>,
    /// Canonical element names per dimension.  Empty for indexed dims.
    pub dim_elements: Vec<Vec<String>>,
    /// True at index `i` if `dimensions[i]` is an indexed dimension
    /// (1..=size integer subscripts), false if named.
    pub is_indexed: Vec<bool>,
    /// Cached size of each dimension; product equals `n_slots`.
    pub dim_sizes: Vec<usize>,
    /// Total slot count occupied by this loop's `loop_score` series.
    /// Scalar loops have `n_slots = 1` and empty `dimensions`.
    pub n_slots: usize,
}

/// Subscript-resolution failures for [`LoopElementIndex::resolve`].
///
/// All error variants carry enough context for a human-readable FFI
/// error message (e.g. "loop r1 dimension 'region' has no element
/// 'tokyo'").  Names and values are returned in canonical (lowercased)
/// form, matching the index's storage convention.
#[derive(Debug, PartialEq, Eq)]
pub enum ResolveError {
    DimCountMismatch {
        expected: usize,
        got: usize,
    },
    ElementNotFound {
        dim: String,
        value: String,
    },
    IndexOutOfRange {
        dim: String,
        value: String,
        max: usize,
    },
    InvalidIntegerSubscript {
        dim: String,
        value: String,
    },
}

impl LoopElementIndex {
    /// Resolve an N-tuple of subscripts to a linear slot offset within
    /// this loop's `loop_score` series.
    ///
    /// Layout convention is row-major (last-dim-fastest): for a 2D
    /// `[d_0, d_1]` loop with sizes `[s_0, s_1]`, the slot for element
    /// `[i_0, i_1]` lives at offset `i_0 * s_1 + i_1`.
    ///
    /// Subscripts are accepted in raw form; canonicalization happens
    /// internally so callers don't need to pre-lowercase.  For named
    /// dimensions the subscript matches against the canonical element
    /// name; for indexed dimensions it is parsed as a 1..=size integer.
    ///
    /// A scalar loop (`n_slots == 1`, `dimensions` empty) accepts an
    /// empty subscript list and returns offset 0.  Any subscripted
    /// access on a scalar loop yields `DimCountMismatch`.
    pub fn resolve(&self, subscripts: &[&str]) -> Result<usize, ResolveError> {
        use crate::canonicalize;

        if subscripts.len() != self.dimensions.len() {
            return Err(ResolveError::DimCountMismatch {
                expected: self.dimensions.len(),
                got: subscripts.len(),
            });
        }
        if subscripts.is_empty() {
            return Ok(0);
        }
        let mut linear: usize = 0;
        for (i, raw) in subscripts.iter().enumerate() {
            let canon = canonicalize(raw).into_owned();
            let dim_name = &self.dimensions[i];
            let size = self.dim_sizes[i];
            let per_dim_offset = if self.is_indexed[i] {
                // Indexed dimension: subscript is a 1..=size integer.
                let parsed: u32 =
                    canon
                        .parse()
                        .map_err(|_| ResolveError::InvalidIntegerSubscript {
                            dim: dim_name.clone(),
                            value: canon.clone(),
                        })?;
                if parsed < 1 || (parsed as usize) > size {
                    return Err(ResolveError::IndexOutOfRange {
                        dim: dim_name.clone(),
                        value: canon,
                        max: size,
                    });
                }
                (parsed - 1) as usize
            } else {
                // Named dimension: linear search canonical element list.
                self.dim_elements[i]
                    .iter()
                    .position(|name| name == &canon)
                    .ok_or_else(|| ResolveError::ElementNotFound {
                        dim: dim_name.clone(),
                        value: canon,
                    })?
            };
            linear = linear * size + per_dim_offset;
        }
        Ok(linear)
    }
}

const LOOP_SCORE_PREFIX: &str = "$\u{205A}ltm\u{205A}loop_score\u{205A}";

/// Build a per-loop-id index of dimension metadata from the LTM
/// variable list emitted by `model_ltm_variables` and the project's
/// declared dimensions.
///
/// Only entries whose name starts with `$⁚ltm⁚loop_score⁚` are
/// indexed; link_score, path, and composite variables are filtered
/// out (they're not exposed as loop IDs to FFI consumers).
///
/// Dimensions and element names are canonicalized via
/// [`crate::canonicalize`] so the FFI resolver can do direct string
/// comparisons against user input that's also been canonicalized.
/// Dimensions referenced by an LTM var that aren't present in
/// `project_dims` are silently skipped: in practice this only
/// happens for malformed inputs, and the resolver naturally fails
/// (n_slots reflects only the resolved dims).
pub fn build_loop_element_index(
    ltm_vars: &[crate::db::LtmSyntheticVar],
    project_dims: &[crate::datamodel::Dimension],
) -> HashMap<String, LoopElementIndex> {
    use crate::canonicalize;

    // Pre-canonicalize project dimension names + elements once so each
    // LTM var's lookup is O(d) instead of re-canonicalizing per dim per
    // entry.
    let dim_lookup: HashMap<String, &crate::datamodel::Dimension> = project_dims
        .iter()
        .map(|d| (canonicalize(d.name()).into_owned(), d))
        .collect();

    let mut out: HashMap<String, LoopElementIndex> = HashMap::new();
    for var in ltm_vars {
        let Some(loop_id) = var.name.strip_prefix(LOOP_SCORE_PREFIX) else {
            continue;
        };
        let mut dimensions = Vec::with_capacity(var.dimensions.len());
        let mut dim_elements = Vec::with_capacity(var.dimensions.len());
        let mut is_indexed = Vec::with_capacity(var.dimensions.len());
        let mut dim_sizes = Vec::with_capacity(var.dimensions.len());
        for raw_dim_name in &var.dimensions {
            let canonical_dim = canonicalize(raw_dim_name).into_owned();
            let Some(dim) = dim_lookup.get(&canonical_dim).copied() else {
                continue;
            };
            let elements: Vec<String> = if dim.is_indexed() {
                Vec::new()
            } else {
                use crate::datamodel::DimensionElements;
                match &dim.elements {
                    DimensionElements::Named(names) => {
                        names.iter().map(|n| canonicalize(n).into_owned()).collect()
                    }
                    DimensionElements::Indexed(_) => Vec::new(),
                }
            };
            dimensions.push(canonical_dim);
            dim_elements.push(elements);
            is_indexed.push(dim.is_indexed());
            dim_sizes.push(dim.len());
        }
        let n_slots: usize = if dim_sizes.is_empty() {
            1
        } else {
            dim_sizes.iter().product::<usize>().max(1)
        };
        out.insert(
            loop_id.to_string(),
            LoopElementIndex {
                dimensions,
                dim_elements,
                is_indexed,
                dim_sizes,
                n_slots,
            },
        );
    }
    out
}

/// Aggregate the per-element rel-score map produced by
/// [`compute_rel_loop_scores_per_element`] into a single signed series
/// per loop, via signed argmax-abs across each loop's own slots.
///
/// Used by the layout's `compute_metadata` to populate
/// `FeedbackLoop::importance_series` with one value per saved step.
///
/// ## Stride handling
///
/// [`compute_rel_loop_scores_per_element`] lays each loop's series out
/// row-major as `series[t * stride + k]`, where:
///
/// - for an **arrayed** loop, `stride == n_slots` -- the loop's own
///   slot count.  Every slot index in `0..n_slots` is a real element,
///   so there are no padding positions and `n == stride`.
/// - for a **scalar** loop, `stride` is the largest slot index its
///   (slot-0) partition covers + 1 (1 if no arrayed loop shares that
///   partition); the loop's own `n_slots` is 1, so `stride >= n` with
///   positions `1..stride` being broadcast padding the scalar loop's
///   own value never occupies.
///
/// This helper recovers `stride` from `series.len() / step_count` so
/// consumers don't need to track partition stride independently.
///
/// The inner argmax-abs iterates only the loop's *own* `n_slots`
/// (`n_slots_by_loop[loop_id]`, default 1), reading `series[t * stride + k]`
/// for `k` in `0..n_slots`.  For a scalar loop that is slot 0 only --
/// the canonical scalar view, matching the pre-PR `compute_rel_loop_scores`
/// behaviour; the partition's broadcast-padding positions `1..stride`
/// are skipped.  For an arrayed loop `stride == n_slots`, so the loop
/// over `0..n_slots` covers exactly the loop's own elements with no
/// out-of-bounds read.  (Pre-Phase-2, arrayed loops were padded to the
/// partition's max stride and had to skip `n_slots..stride`; that
/// padding no longer exists.)
///
/// ## Output
///
/// Per loop: a `Vec<f64>` of length `step_count` (or empty if the
/// input series is empty).  Non-finite picks are mapped to `0.0`,
/// matching the existing layout filter.
///
/// Loops present in `per_element_rel_scores` but absent from
/// `n_slots_by_loop` default to `n_slots = 1` (scalar) -- legacy
/// callers that haven't snapshotted dim metadata still get a
/// well-formed result.
pub fn aggregate_per_element_argmax_abs(
    per_element_rel_scores: &HashMap<String, Vec<f64>>,
    n_slots_by_loop: &HashMap<String, usize>,
    step_count: usize,
) -> HashMap<String, Vec<f64>> {
    let mut out = HashMap::with_capacity(per_element_rel_scores.len());
    for (loop_id, series) in per_element_rel_scores {
        if series.is_empty() {
            out.insert(loop_id.clone(), Vec::new());
            continue;
        }
        let n = n_slots_by_loop.get(loop_id).copied().unwrap_or(1).max(1);
        // Recover the helper's actual stride from the input length.
        // For a scalar loop in a mixed partition stride > n == 1
        // (broadcast padding); for an arrayed loop and for a scalar
        // loop alone in its partition stride == n.
        let stride = (series.len() / step_count.max(1)).max(1);
        let mut agg = Vec::with_capacity(step_count);
        for t in 0..step_count {
            let mut best = 0.0_f64;
            let mut best_abs = -1.0_f64;
            // Iterate this loop's own slots only.  `n <= stride` always
            // by construction (an arrayed loop has stride == n_slots; a
            // scalar loop has n == 1 and stride >= 1), so the index
            // never exceeds the series bounds.
            for k in 0..n {
                let v = series[t * stride + k];
                // `>` (not `>=`) keeps lowest-index slot on ties; NaN
                // comparisons are always false so a NaN never displaces
                // a finite candidate.
                if v.abs() > best_abs {
                    best_abs = v.abs();
                    best = v;
                }
            }
            agg.push(if best.is_finite() { best } else { 0.0 });
        }
        out.insert(loop_id.clone(), agg);
    }
    out
}

/// Aggregate a multi-slot loop's relative-score series down to a single
/// signed series by picking the element with the largest `|rel[k, t]|`
/// at each step and emitting that element's *signed* value.
///
/// Sign is preserved across steps even when the dominant element flips
/// (e.g. argmax may be slot 0 at one step and slot 1 at the next).
/// Ties on `|rel|` are broken by lowest slot index, matching the
/// stable-first-wins convention `max_by_key` provides.
///
/// `denominators_per_element` must have length `n_slots`, with each
/// entry's length equal to `results.step_count`.  The element-`k`
/// denominator is the partition sum at element `k` (e.g. produced by
/// [`compute_partition_denominator_for_element`] called with the
/// member loops and `element_index = k`).
///
/// Scalar (`n_slots == 1`) reduces to identity: the aggregator returns
/// the same series as [`compute_rel_loop_score_for_id`] would.  Used
/// by both the layout single-line importance metric and the FFI
/// dispatch when callers pass a bare arrayed loop ID without a
/// subscript.
pub fn compute_rel_loop_score_argmax_abs(
    results: &Results,
    loop_id: &str,
    n_slots: usize,
    denominators_per_element: &[&[f64]],
) -> Option<Vec<f64>> {
    let off = results.offsets.get(&loop_score_ident(loop_id)).copied()?;
    let slots = n_slots.max(1);
    debug_assert_eq!(
        denominators_per_element.len(),
        slots,
        "argmax-abs needs one denominator series per element slot"
    );
    let mut out = Vec::with_capacity(results.step_count);
    for (t, row) in results.iter().enumerate() {
        let mut best: f64 = 0.0;
        let mut best_abs: f64 = -1.0;
        for (k, denom_series) in denominators_per_element.iter().take(slots).enumerate() {
            // `k` is iterated 0..slots and slots == max(n_slots, 1), so
            // `effective_slot` always returns `Some`.  We pass it through
            // anyway for consistency with the streaming helpers and to
            // catch any future caller that constructs `denominators_per_element`
            // longer than the loop's own `n_slots`.
            let Some(slot) = effective_slot(n_slots, k) else {
                continue;
            };
            let num = row[off + slot];
            let denom = denom_series[t];
            let rel = if denom == 0.0 { 0.0 } else { num / denom };
            // `>` (not `>=`) keeps the lowest-index slot when ties occur.
            if rel.abs() > best_abs {
                best_abs = rel.abs();
                best = rel;
            }
        }
        out.push(best);
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

    /// Build a per-loop, single-slot `loop_partitions` mapping from
    /// `(loop_id, partition)` pairs -- the common scalar/cross-element case
    /// where every loop has exactly one slot.
    fn mapping(pairs: &[(&str, Option<usize>)]) -> HashMap<String, Vec<Option<usize>>> {
        pairs
            .iter()
            .map(|(id, p)| ((*id).to_string(), vec![*p]))
            .collect()
    }

    /// Build a per-slot `loop_partitions` mapping from `(loop_id, slots)`
    /// pairs -- for tests that need genuinely multi-slot A2A loops.
    fn mapping_per_slot(
        pairs: &[(&str, Vec<Option<usize>>)],
    ) -> HashMap<String, Vec<Option<usize>>> {
        pairs
            .iter()
            .map(|(id, slots)| ((*id).to_string(), slots.clone()))
            .collect()
    }

    /// Inlined reference implementation of `compute_rel_loop_scores`'s
    /// slot-0-convenience SAFEDIV formula: each loop normalizes its slot-0
    /// value against the sum of slot-0 values over loops sharing its slot-0
    /// partition.  The proptest compares against this to catch any numeric
    /// divergence.
    ///
    /// `NaN` summands are excluded from the partition sum here too (GH
    /// #542), independently re-deriving the production `denom_summand`
    /// rule, so the oracle stays correct if a future generator introduces
    /// non-finite samples (the current generators are finite-only, so this
    /// branch is latent but kept honest).  `+/-Inf` is kept in the sum,
    /// matching the production semantics.
    fn reference_rel_loop_scores(
        loop_ids: &[String],
        loop_partitions: &HashMap<String, Vec<Option<usize>>>,
        series: &[Vec<f64>],
    ) -> Vec<Vec<f64>> {
        let step_count = series.first().map(|s| s.len()).unwrap_or(0);
        let mut groups: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
        for (i, id) in loop_ids.iter().enumerate() {
            let key = loop_partitions
                .get(id)
                .and_then(|v| v.first().copied().flatten());
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
                let denom: f64 = indices
                    .iter()
                    .map(|&i| {
                        let v = series[i][t];
                        if v.is_nan() { 0.0 } else { v.abs() }
                    })
                    .sum();
                for &i in indices {
                    let num = series[i][t];
                    let val = if denom == 0.0 { 0.0 } else { num / denom };
                    out[i].push(val);
                }
            }
        }
        out
    }

    /// Naive, from-first-principles reference for
    /// `compute_rel_loop_scores_per_element`.  Where the engine pre-builds
    /// a `(partition, slot)` BTreeMap grid, this loops directly over each
    /// `(loop, output-slot, step)` and re-derives the SAFEDIV denominator
    /// by scanning *every* loop and asking "is it a member of this bucket?"
    /// -- a structurally different computation, so the proptest is a real
    /// oracle, not a paraphrase of the implementation.
    ///
    /// Member rule (mirrors the engine's `slot_partition` broadcast and
    /// `effective_slot` gating, spelled out inline rather than via the
    /// engine helpers): for a bucket at partition `p`, output slot `k`,
    /// loop `j` is a member iff
    ///   - `j` is **scalar** (`n_slots == 1`): `slots[j][0] == p` -- it
    ///     broadcasts its single value into every slot of partition `p`;
    ///     it reads slot 0; OR
    ///   - `j` is **arrayed** (`n_slots > 1`) and `k < n_slots[j]` and
    ///     `slots[j][k] == p` -- it reads its own slot `k`; an arrayed
    ///     loop past its own slot count is not a member of slot-`k`
    ///     buckets (no out-of-bounds read into another loop's data).
    ///
    /// Output stride per loop: an arrayed loop's own slot count; a scalar
    /// loop's (slot-0) partition's largest covered slot index + 1 (so a
    /// scalar loop alone in its partition has stride 1).  Slots a scalar
    /// loop's partition does not cover stay 0.0 (gaps in the covered set).
    ///
    /// `slots[i]` is loop `i`'s per-slot partition vector (length 1 for a
    /// scalar loop); `series[i][step][slot]` its `loop_score`.  Returns one
    /// flat `Vec<f64>` per loop in `loop_ids` order; element `step *
    /// stride_i + k`.  Float summation walks loops in ascending index --
    /// the same order the engine's sorted-`loop_id` member lists produce
    /// when the ids are `L{i}` with `i < 10` -- so the comparison is exact.
    fn reference_rel_loop_scores_per_element(
        loop_ids: &[String],
        slots: &[Vec<Option<usize>>],
        series: &[Vec<Vec<f64>>],
        step_count: usize,
    ) -> Vec<Vec<f64>> {
        let n = loop_ids.len();
        let n_slots: Vec<usize> = slots.iter().map(|v| v.len().max(1)).collect();
        // The partition loop `i` carries into slot `k`: an arrayed loop's
        // own per-slot entry; a scalar loop broadcasts slot 0's partition.
        let slot_part = |i: usize, k: usize| -> Option<usize> {
            if n_slots[i] <= 1 {
                slots[i].first().copied().flatten()
            } else {
                slots[i].get(k).copied().flatten()
            }
        };
        // For each partition, the set of slot indices some loop occupies in
        // it (a scalar loop occupies only slot 0): used for the scalar
        // broadcast stride and to know whether a scalar loop "appears" at a
        // given output slot.
        let mut partition_slots: BTreeMap<Option<usize>, BTreeSet<usize>> = BTreeMap::new();
        for (i, &ns) in n_slots.iter().enumerate() {
            for k in 0..ns {
                partition_slots
                    .entry(slot_part(i, k))
                    .or_default()
                    .insert(k);
            }
        }
        let strides: Vec<usize> = (0..n)
            .map(|i| {
                if n_slots[i] > 1 {
                    n_slots[i]
                } else {
                    let p = slot_part(i, 0);
                    partition_slots
                        .get(&p)
                        .and_then(|ks| ks.iter().max().copied())
                        .map(|m| m + 1)
                        .unwrap_or(1)
                        .max(1)
                }
            })
            .collect();
        // Is loop `j` a member of the bucket (partition `p`, output slot
        // `k`)?  If so, which of its own slots does it read?
        //
        // A scalar loop is in `(p, k)` iff its (slot-0) partition is `p`
        // AND `k` is a slot index *some* loop occupies in `p`
        // (`partition_slots[p]`) -- the engine only pushes a scalar member
        // into the slots its partition actually spans, so a stride that
        // overshoots a gap leaves that output position at 0.0.
        let read_slot_in_bucket = |j: usize, p: Option<usize>, k: usize| -> Option<usize> {
            if n_slots[j] <= 1 {
                let covers_k = partition_slots.get(&p).is_some_and(|ks| ks.contains(&k));
                if slot_part(j, 0) == p && covers_k {
                    Some(0)
                } else {
                    None
                }
            } else if k < n_slots[j] && slot_part(j, k) == p {
                Some(k)
            } else {
                None
            }
        };
        let mut out: Vec<Vec<f64>> = (0..n)
            .map(|i| vec![0.0_f64; step_count * strides[i]])
            .collect();
        for i in 0..n {
            for k in 0..strides[i] {
                // Which bucket is loop `i`'s output slot `k` in, and does
                // `i` actually occupy it?  An arrayed loop occupies every
                // `k < n_slots` (and stride == n_slots, so no overshoot);
                // a scalar loop occupies only the slots its partition
                // covers (its stride may overshoot a gap).
                let p = slot_part(i, k);
                let Some(read_i) = read_slot_in_bucket(i, p, k) else {
                    continue;
                };
                // Bucket members, scanned over every loop.
                let bucket: Vec<(usize, usize)> = (0..n)
                    .filter_map(|j| read_slot_in_bucket(j, p, k).map(|rs| (j, rs)))
                    .collect();
                for step in 0..step_count {
                    // `NaN` summands excluded (GH #542), re-derived inline
                    // so this oracle stays structurally independent of the
                    // production `denom_summand`; `Inf` kept in the sum.
                    let denom: f64 = bucket
                        .iter()
                        .map(|&(j, rs)| {
                            let v = series[j][step][rs];
                            if v.is_nan() { 0.0 } else { v.abs() }
                        })
                        .sum();
                    let num = series[i][step][read_i];
                    let val = if denom == 0.0 { 0.0 } else { num / denom };
                    out[i][step * strides[i] + k] = val;
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
    fn nan_loop_score_isolated_to_its_own_loop() {
        // A single NaN loop_score must NOT poison the whole partition's
        // relative scores (GH #542).  A NaN summand is excluded from the
        // partition denominator, so a healthy sibling still normalizes
        // against the healthy denominator; only the loop whose own
        // numerator is NaN keeps a NaN relative score (the honest "this
        // one loop is undefined here" signal).  This also matches the
        // discovery path's "NaN link contributes nothing" philosophy
        // (`ltm_finding::SearchGraph::from_edges` coerces NaN -> 0).
        let nan = f64::NAN;
        let series_a = &[nan, 2.0][..];
        let series_b = &[1.0, 3.0][..];
        let results = make_results_for_loops(&[("A", series_a), ("B", series_b)]);
        let partitions = mapping(&[("A", Some(0)), ("B", Some(0))]);

        let scored = compute_rel_loop_scores(&results, &partitions);
        let rel_a = scored.get("A").unwrap();
        let rel_b = scored.get("B").unwrap();

        // t=0: the NaN summand is dropped, so denom = |1| = 1.  The bad
        // loop A's own numerator is NaN -> NaN/1 = NaN (its own signal);
        // the healthy loop B is unaffected -> 1/1 = 1.
        assert!(rel_a[0].is_nan(), "the NaN loop's own rel score stays NaN");
        assert!(
            (rel_b[0] - 1.0).abs() < 1e-12,
            "healthy loop normalizes against the healthy denom, not NaN: got {}",
            rel_b[0]
        );
        // t=1: well-defined; denom = 2 + 3 = 5.
        assert!((rel_a[1] - 0.4).abs() < 1e-12);
        assert!((rel_b[1] - 0.6).abs() < 1e-12);
    }

    #[test]
    fn nan_loop_score_isolated_in_per_element_bucket() {
        // Per-element twin of `nan_loop_score_isolated_to_its_own_loop`:
        // a NaN at one (loop, slot) must not poison the sibling sharing
        // that `(partition, slot)` bucket.  Two coupled A2A loops, 2
        // slots each; plant a NaN at A's slot 0 at step 0.
        let n_slots: usize = 2;
        // A slot0 = [NaN, 9], A slot1 = [4, 4]
        // B slot0 = [3,   3], B slot1 = [6, 6]
        let loop_data = vec![
            vec![vec![f64::NAN, 4.0], vec![9.0, 4.0]],
            vec![vec![3.0, 6.0], vec![3.0, 6.0]],
        ];
        let results = make_arrayed_results(&["A", "B"], &[n_slots, n_slots], &loop_data);
        let partitions =
            mapping_per_slot(&[("A", vec![Some(0); n_slots]), ("B", vec![Some(0); n_slots])]);

        let rel = compute_rel_loop_scores_per_element(&results, &partitions);
        let a = rel.get("A").unwrap();
        let b = rel.get("B").unwrap();

        // step 0, slot 0: bucket {A, B}; A is NaN, so it is dropped from
        // the denom (denom = |3| = 3).  A's own rel score = NaN/3 = NaN;
        // B's = 3/3 = 1 (healthy, NOT poisoned).
        let at = |step: usize, k: usize| step * n_slots + k;
        assert!(a[at(0, 0)].is_nan(), "NaN loop keeps its own NaN rel score");
        assert!(
            (b[at(0, 0)] - 1.0).abs() < 1e-12,
            "healthy slot-0 sibling normalizes against the healthy denom: got {}",
            b[at(0, 0)]
        );
        // step 0, slot 1: both finite; denom = |4| + |6| = 10.
        assert!((a[at(0, 1)] - 0.4).abs() < 1e-12);
        assert!((b[at(0, 1)] - 0.6).abs() < 1e-12);
        // step 1: all finite; slot 0 denom = |9| + |3| = 12, slot 1 = 10.
        assert!((a[at(1, 0)] - (9.0 / 12.0)).abs() < 1e-12);
        assert!((b[at(1, 0)] - (3.0 / 12.0)).abs() < 1e-12);
    }

    #[test]
    fn inf_loop_score_kept_in_denominator() {
        // An +Inf loop_score is REAL signal: at a dominance inflection a
        // raw loop score legitimately diverges (the link-score
        // denominators go to zero there).  Unlike a NaN, an Inf is NOT
        // filtered from the partition sum -- it stays, so the dominated
        // siblings correctly go to 0 (finite/Inf) and the dominant loop
        // momentarily reads NaN (Inf/Inf).  This preserves the legitimate
        // inflection-point behaviour of the removed SAFEDIV equation
        // bug-for-bug; only the NaN-poisoning case (GH #542) changed.
        let inf = f64::INFINITY;
        let series_a = &[inf, 2.0][..];
        let series_b = &[5.0, 3.0][..];
        let results = make_results_for_loops(&[("A", series_a), ("B", series_b)]);
        let partitions = mapping(&[("A", Some(0)), ("B", Some(0))]);

        let scored = compute_rel_loop_scores(&results, &partitions);
        let rel_a = scored.get("A").unwrap();
        let rel_b = scored.get("B").unwrap();

        // t=0: denom = |Inf| + |5| = Inf (Inf kept in the sum).
        //   dominant loop A: Inf/Inf = NaN.
        //   dominated loop B: 5/Inf = 0 (it does not matter when a
        //   sibling is infinitely dominant).
        assert!(
            rel_a[0].is_nan(),
            "the dominant +Inf loop reads NaN at the inflection"
        );
        assert_eq!(rel_b[0], 0.0, "a loop dominated by an +Inf sibling -> 0");
        // t=1: finite; denom = 2 + 3 = 5.
        assert!((rel_a[1] - 0.4).abs() < 1e-12);
        assert!((rel_b[1] - 0.6).abs() < 1e-12);
    }

    #[test]
    fn inf_loop_score_kept_in_per_element_bucket() {
        // Per-element twin of `inf_loop_score_kept_in_denominator`: an
        // +Inf at one (loop, slot) stays in that bucket's denominator,
        // so the dominated sibling -> 0 and the dominant loop -> NaN.
        let n_slots: usize = 1;
        // A slot0 = [Inf], B slot0 = [5].
        let loop_data = vec![vec![vec![f64::INFINITY]], vec![vec![5.0]]];
        let results = make_arrayed_results(&["A", "B"], &[n_slots, n_slots], &loop_data);
        let partitions =
            mapping_per_slot(&[("A", vec![Some(0); n_slots]), ("B", vec![Some(0); n_slots])]);

        let rel = compute_rel_loop_scores_per_element(&results, &partitions);
        let a = rel.get("A").unwrap();
        let b = rel.get("B").unwrap();
        assert!(a[0].is_nan(), "dominant +Inf loop -> NaN in its bucket");
        assert_eq!(b[0], 0.0, "dominated sibling -> 0 in the same bucket");
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

    /// The scalar streaming denominator (`compute_partition_denominator`)
    /// excludes `NaN` and keeps `Inf`, so the FFI's amortized scalar path
    /// isolates a NaN loop the same way the full-sweep helper does
    /// (GH #542).
    #[test]
    fn per_id_streaming_denominator_excludes_nan_keeps_inf() {
        // t0: A = NaN, B = 1 -> denom = 1 (NaN dropped).
        // t1: A = Inf, B = 1 -> denom = Inf (Inf kept).
        let series_a = &[f64::NAN, f64::INFINITY][..];
        let series_b = &[1.0, 1.0][..];
        let results = make_results_for_loops(&[("A", series_a), ("B", series_b)]);

        let denom = compute_partition_denominator(&results, ["A", "B"]);
        assert_eq!(denom, vec![1.0, f64::INFINITY]);

        // Healthy B: 1/1 = 1 at t0 (not poisoned), 1/Inf = 0 at t1.
        let rel_b = compute_rel_loop_score_for_id(&results, "B", &denom).unwrap();
        assert!(
            (rel_b[0] - 1.0).abs() < 1e-12,
            "healthy B not poisoned: {}",
            rel_b[0]
        );
        assert_eq!(rel_b[1], 0.0, "B dominated by +Inf sibling -> 0");
        // The bad loop A keeps its own NaN at t0; at t1 Inf/Inf = NaN.
        let rel_a = compute_rel_loop_score_for_id(&results, "A", &denom).unwrap();
        assert!(rel_a[0].is_nan());
        assert!(rel_a[1].is_nan());
    }

    /// Per-element variant: two A2A loops over an element-wise-coupled
    /// dimension (every slot in partition 0), each with 3 element slots.
    /// At every element k both loops' slot k lands in bucket `(0, k)`, so
    /// the sum of absolute rel-scores at element k must equal 1.0 (non-zero
    /// elements) or 0.0 (zero-denominator elements) independently -- the
    /// scalar path collapses to slot 0 and would sum to 1.0 only for
    /// element 0.
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

        // Both loops coupled: every slot in partition 0.
        let partitions =
            mapping_per_slot(&[("A", vec![Some(0); n_slots]), ("B", vec![Some(0); n_slots])]);

        let rel = compute_rel_loop_scores_per_element(&results, &partitions);
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

    /// Per-element variant, the headline GH #487 case: two A2A loops over
    /// element-wise-*uncoupled* dimensions -- each slot of each loop is in
    /// its own partition, and no slot of A shares a partition with any slot
    /// of B.  Each loop's slot k therefore normalizes against itself only,
    /// so every rel score is ±1.0 -- the two loops do NOT cross-normalize
    /// even though `compute_rel_loop_scores`'s slot-0-pooled view used to
    /// (pre-fix) lump them when both had `None` partitions.
    #[test]
    fn per_element_uncoupled_a2a_loops_do_not_cross_normalize() {
        let step_count: usize = 3;
        // A has 2 slots, B has 3 slots; A's slots are partitions 0,1 and
        // B's slots are partitions 2,3,4 -- all distinct, none shared.
        // Layout: time | A slot0 | A slot1 | B slot0..2
        let step_size = 1 + 2 + 3;
        let a_off = 1;
        let b_off = 3;
        let mut data = vec![0.0_f64; step_count * step_size];
        for step in 0..step_count {
            let row = &mut data[step * step_size..(step + 1) * step_size];
            row[0] = step as f64;
            // Distinct, non-zero, per-step-varying magnitudes.
            for k in 0..2 {
                row[a_off + k] = ((step + 2) * (k + 1)) as f64;
            }
            for k in 0..3 {
                row[b_off + k] = -(((step + 3) * (k + 1)) as f64);
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

        let partitions = mapping_per_slot(&[
            ("A", vec![Some(0), Some(1)]),
            ("B", vec![Some(2), Some(3), Some(4)]),
        ]);
        let rel = compute_rel_loop_scores_per_element(&results, &partitions);
        let a = rel.get("A").expect("A must have a series");
        let b = rel.get("B").expect("B must have a series");
        assert_eq!(a.len(), step_count * 2);
        assert_eq!(b.len(), step_count * 3);
        // Every slot of every loop normalizes against itself only -> ±1.0.
        for &v in a.iter().chain(b.iter()) {
            assert!(
                (v.abs() - 1.0).abs() < 1e-12,
                "uncoupled A2A slot should self-normalize to ±1.0, got {v}"
            );
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

        // A is scalar (one slot in partition 0); B is A2A coupled (both
        // slots in partition 0).  A broadcasts its single value into both
        // of B's slots, so A's series is padded to B's stride.
        let partitions = mapping_per_slot(&[("A", vec![Some(0)]), ("B", vec![Some(0), Some(0)])]);

        let rel = compute_rel_loop_scores_per_element(&results, &partitions);
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

    /// `aggregate_per_element_argmax_abs`: pure-arrayed sanity check.
    /// Single arrayed loop with 3 elements, stride==n; output should
    /// be argmax-abs across the 3 elements at each step.
    #[test]
    fn aggregate_pure_arrayed_argmax_abs() {
        let mut per_elem = HashMap::new();
        // 2 steps × 3 elements: layout step*3 + k.
        //   step 0: [0.1,  0.5, -0.2]  -> argmax-abs picks 0.5
        //   step 1: [0.3, -0.4, 0.0]   -> argmax-abs picks -0.4
        per_elem.insert("L".to_string(), vec![0.1, 0.5, -0.2, 0.3, -0.4, 0.0]);
        let mut n_slots = HashMap::new();
        n_slots.insert("L".to_string(), 3);

        let out = aggregate_per_element_argmax_abs(&per_elem, &n_slots, 2);
        let agg = out.get("L").expect("L must have aggregate");
        assert_eq!(agg, &vec![0.5, -0.4]);
    }

    /// `aggregate_per_element_argmax_abs`: scalar in mixed partition.
    /// The helper input has stride=3 (partition max) but n=1 for the
    /// scalar loop.  Output should have length step_count, with each
    /// value taken from the loop's own slot 0 (canonical scalar view).
    /// Pre-fix the layout used the wrong stride and produced a series
    /// of length step_count*3 with misaligned values.
    #[test]
    fn aggregate_scalar_in_mixed_partition_returns_step_count_values() {
        let mut per_elem = HashMap::new();
        // Scalar A in mixed partition (stride=3); 4 steps × 3 elements.
        // Each step has the same value at index 0 (slot 0 broadcast),
        // but indices 1,2 carry the per-element rel-scores from
        // distinct partition denominators.
        //   step 0: [0.10, 0.20, 0.30]
        //   step 1: [0.15, 0.25, 0.35]
        //   step 2: [0.12, 0.22, 0.32]
        //   step 3: [0.18, 0.28, 0.38]
        per_elem.insert(
            "A".to_string(),
            vec![
                0.10, 0.20, 0.30, // step 0
                0.15, 0.25, 0.35, // step 1
                0.12, 0.22, 0.32, // step 2
                0.18, 0.28, 0.38, // step 3
            ],
        );
        let mut n_slots = HashMap::new();
        n_slots.insert("A".to_string(), 1);

        let out = aggregate_per_element_argmax_abs(&per_elem, &n_slots, 4);
        let agg = out.get("A").expect("A must have aggregate");
        // Length must match step_count, NOT step_count*stride.  Each
        // value is the scalar's own slot 0 at that step.
        assert_eq!(agg.len(), 4);
        assert_eq!(agg, &vec![0.10, 0.15, 0.12, 0.18]);
    }

    /// `aggregate_per_element_argmax_abs`: the recovered stride and the
    /// mapped `n_slots` need not agree.  Post-Phase-2
    /// `compute_rel_loop_scores_per_element` lays an arrayed loop out at
    /// `stride == n_slots`, but the aggregator is defensive: if a caller
    /// supplies a series whose recovered stride (5 here) exceeds the
    /// loop's mapped `n_slots` (2) -- e.g. a partially-snapshotted
    /// `n_slots_by_loop` -- the argmax-abs iterates only the mapped 2
    /// slots, never the trailing padding.
    #[test]
    fn aggregate_arrayed_in_mixed_partition_iterates_own_slots_only() {
        let mut per_elem = HashMap::new();
        // 2 steps × 5-stride.  Loop is mapped to n=2 own slots.
        // Positions 2..5 are stale/padding and must NOT be included in
        // argmax-abs.  To prove the iterator stops at n=2, place a trap
        // value (1e6) at position 4 -- if the helper ever iterates
        // 0..stride it would pick this and we'd notice.
        per_elem.insert(
            "B".to_string(),
            vec![
                0.1, -0.3, 0.0, 0.0, 1.0e6, // step 0; trap at index 4
                0.4, 0.2, 0.0, 0.0, 1.0e6, // step 1; trap at index 4
            ],
        );
        let mut n_slots = HashMap::new();
        n_slots.insert("B".to_string(), 2);

        let out = aggregate_per_element_argmax_abs(&per_elem, &n_slots, 2);
        let agg = out.get("B").expect("B must have aggregate");
        assert_eq!(agg.len(), 2);
        // step 0: argmax-abs over [0.1, -0.3] = -0.3 (sign preserved).
        // step 1: argmax-abs over [0.4,  0.2] = 0.4.
        assert_eq!(agg, &vec![-0.3, 0.4]);
    }

    /// Non-finite values (NaN, ±Inf) get mapped to 0.0 in the output,
    /// matching the existing layout filter behavior.
    #[test]
    fn aggregate_filters_non_finite() {
        let mut per_elem = HashMap::new();
        per_elem.insert(
            "L".to_string(),
            vec![f64::NAN, 0.5, f64::INFINITY, -f64::INFINITY],
        );
        let mut n_slots = HashMap::new();
        n_slots.insert("L".to_string(), 2);

        let out = aggregate_per_element_argmax_abs(&per_elem, &n_slots, 2);
        let agg = out.get("L").expect("L must have aggregate");
        // step 0: [NaN, 0.5].  argmax-abs comparison with NaN is false,
        //   so 0.5 wins.  Output is finite (0.5).
        // step 1: [Inf, -Inf].  Both non-finite; output 0.0.
        assert_eq!(agg.len(), 2);
        assert_eq!(agg[0], 0.5);
        assert_eq!(agg[1], 0.0);
    }

    /// An empty per-loop series yields an empty aggregate (matches
    /// the existing layout's "no series" branch).
    #[test]
    fn aggregate_empty_series_yields_empty_output() {
        let mut per_elem = HashMap::new();
        per_elem.insert("L".to_string(), Vec::new());
        let mut n_slots = HashMap::new();
        n_slots.insert("L".to_string(), 1);

        let out = aggregate_per_element_argmax_abs(&per_elem, &n_slots, 5);
        let agg = out.get("L").expect("L must have aggregate (even if empty)");
        assert!(agg.is_empty());
    }

    /// Mismatched-dim arrayed partition: loop A has n=2 and loop B has
    /// n=3 in the same partition.  At partition element k=2, A has no
    /// own element; the helper must NOT OOB-read past A's allocated
    /// slots into B's data, and A's series stays its own length (n=2)
    /// rather than being padded to B's.
    ///
    /// We pick B's slot 0 as a sentinel (999.0) so that an OOB read of
    /// `row[off_A + 2]` (which equals `row[off_B + 0]` in our layout)
    /// pulls this clearly-wrong value -- the test fails loudly rather
    /// than passing by accident on whatever uninitialised data the
    /// allocator happens to return.
    #[test]
    fn per_element_helper_handles_arrayed_with_smaller_n() {
        // Layout: time | A slots 0..2 | B slots 0..3
        // step 0:        | 1.0  2.0   | 999.0  20.0  30.0
        let loop_data = vec![vec![vec![1.0, 2.0]], vec![vec![999.0, 20.0, 30.0]]];
        let results = make_arrayed_results(&["A", "B"], &[2, 3], &loop_data);

        // Both coupled: A's two slots and B's three slots all in partition 0.
        let partitions = mapping_per_slot(&[
            ("A", vec![Some(0), Some(0)]),
            ("B", vec![Some(0), Some(0), Some(0)]),
        ]);

        let rel = compute_rel_loop_scores_per_element(&results, &partitions);
        let a = rel.get("A").expect("A must have a series");
        let b = rel.get("B").expect("B must have a series");
        // Each loop's series has its own slot count -- A's is 2, not B's 3.
        assert_eq!(a.len(), 2);
        assert_eq!(b.len(), 3);

        // k=0,1: bucket (0, k) = {A.slotk, B.slotk}.
        //   denom_0 = |1| + |999| = 1000;  a[0] = 1/1000, b[0] = 999/1000.
        //   denom_1 = |2| + |20|  = 22;    a[1] = 2/22,   b[1] = 20/22.
        assert!((a[0] - 1.0 / 1000.0).abs() < 1e-12);
        assert!((b[0] - 999.0 / 1000.0).abs() < 1e-12);
        assert!((a[1] - 2.0 / 22.0).abs() < 1e-12);
        assert!((b[1] - 20.0 / 22.0).abs() < 1e-12);

        // k=2: bucket (0, 2) = {B.slot2} only -- A has no slot 2, so it's
        // not in any slot-2 bucket and doesn't OOB-read into B's data.
        //   denom_2 = |B[2]| = 30 -> b[2] = 30/30 = 1.0.
        assert!(
            (b[2] - 1.0).abs() < 1e-12,
            "B's slot 2 should normalise against itself only (A has no slot 2); got {}",
            b[2]
        );
    }

    /// Build a `Results` with each loop occupying a configurable number of
    /// slots.  Layout: `time | loop0 slot 0..n0 | loop1 slot 0..n1 | ...`.
    /// `loop_data[i][step][slot]` is the value at (step, slot) for loop i.
    fn make_arrayed_results(
        loop_ids: &[&str],
        slots_per_loop: &[usize],
        loop_data: &[Vec<Vec<f64>>],
    ) -> Results {
        assert_eq!(loop_ids.len(), slots_per_loop.len());
        assert_eq!(loop_ids.len(), loop_data.len());
        let step_count = loop_data[0].len();
        for d in loop_data.iter() {
            assert_eq!(d.len(), step_count);
        }
        let total_slots: usize = slots_per_loop.iter().sum();
        let step_size = 1 + total_slots;
        let mut data = vec![0.0_f64; step_count * step_size];
        let mut offsets: HashMap<Ident<Canonical>, usize> = HashMap::new();
        offsets.insert(Ident::new("time"), 0);
        let mut cursor = 1;
        let mut loop_offsets = Vec::with_capacity(loop_ids.len());
        for (i, id) in loop_ids.iter().enumerate() {
            offsets.insert(loop_score_ident(id), cursor);
            loop_offsets.push(cursor);
            cursor += slots_per_loop[i];
        }
        for step in 0..step_count {
            let row = &mut data[step * step_size..(step + 1) * step_size];
            row[0] = step as f64;
            for (i, &off) in loop_offsets.iter().enumerate() {
                let slots = &loop_data[i][step];
                assert_eq!(slots.len(), slots_per_loop[i]);
                for (slot, &v) in slots.iter().enumerate() {
                    row[off + slot] = v;
                }
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

    /// Per-element streaming partition denominator must read the queried
    /// element from each member loop, NOT slot 0.  Two A2A loops with
    /// distinct per-slot values: the denominator at element 1 must equal
    /// `|loop0[t, 1]| + |loop1[t, 1]|`, which differs from element 0's.
    #[test]
    fn per_element_partition_denominator_reads_queried_slot() {
        // 2 loops, 3 slots each, 2 timesteps.
        // loop0[step][slot] and loop1[step][slot] chosen so element-1 values
        // differ visibly from element-0 values.
        let loop_data = vec![
            // loop0: [step][slot]
            vec![
                vec![1.0, 7.0, 2.0], // step 0: slots 0,1,2
                vec![2.0, 9.0, 3.0], // step 1
            ],
            // loop1
            vec![vec![4.0, 5.0, 6.0], vec![8.0, 11.0, 12.0]],
        ];
        let results = make_arrayed_results(&["A", "B"], &[3, 3], &loop_data);

        let denom_e0 = compute_partition_denominator_for_element(
            &results,
            [("A", 3_usize), ("B", 3_usize)],
            0,
        );
        let denom_e1 = compute_partition_denominator_for_element(
            &results,
            [("A", 3_usize), ("B", 3_usize)],
            1,
        );

        // Element 0: |1|+|4|=5, |2|+|8|=10.
        assert_eq!(denom_e0, vec![5.0, 10.0]);
        // Element 1: |7|+|5|=12, |9|+|11|=20.
        assert_eq!(denom_e1, vec![12.0, 20.0]);
        // Sanity: element 1 is genuinely different from element 0.
        assert_ne!(denom_e0, denom_e1);
    }

    /// Scalar loops in mixed partitions must broadcast slot 0 regardless
    /// of the queried element_index.  This matches the pre-PR compile-time
    /// emitter that expanded a scalar loop_score reference across an
    /// arrayed rel_loop_score target.
    #[test]
    fn per_element_partition_denominator_broadcasts_scalar() {
        // Loop A is scalar (1 slot); loop B is A2A (3 slots).
        let loop_data = vec![
            vec![vec![3.0], vec![5.0]],                     // A scalar
            vec![vec![1.0, 7.0, 2.0], vec![2.0, 9.0, 3.0]], // B arrayed
        ];
        let results = make_arrayed_results(&["A", "B"], &[1, 3], &loop_data);

        // Element 0 query: A contributes |3|=3 (its only slot), B contributes |1|=1.
        let denom_e0 = compute_partition_denominator_for_element(
            &results,
            [("A", 1_usize), ("B", 3_usize)],
            0,
        );
        assert_eq!(denom_e0, vec![3.0 + 1.0, 5.0 + 2.0]);

        // Element 1 query: A still contributes |3|=3 (broadcast), B contributes |7|=7.
        let denom_e1 = compute_partition_denominator_for_element(
            &results,
            [("A", 1_usize), ("B", 3_usize)],
            1,
        );
        assert_eq!(denom_e1, vec![3.0 + 7.0, 5.0 + 9.0]);
    }

    /// `compute_rel_loop_score_for_element` paired with
    /// `compute_partition_denominator_for_element` must reproduce the
    /// per-element view that the full-sweep
    /// `compute_rel_loop_scores_per_element` produces.  Bit-for-bit
    /// agreement is the contract the libsimlin per-partition cache
    /// relies on -- the streaming pair is meant to be a strictly cheaper
    /// path to the same numbers, not an approximation.
    #[test]
    fn per_element_streaming_matches_full_sweep() {
        let n_slots: usize = 3;
        let step_count: usize = 4;
        // Reuse the fixture from `per_element_helper_normalizes_within_each_slot`:
        //   A: row[a_off + k] = (step+1) * (k+1)
        //   B: row[b_off + k] = (step+1) * (k+2)
        let mut a_data = Vec::with_capacity(step_count);
        let mut b_data = Vec::with_capacity(step_count);
        for step in 0..step_count {
            let a_row: Vec<f64> = (0..n_slots)
                .map(|k| ((step + 1) * (k + 1)) as f64)
                .collect();
            let b_row: Vec<f64> = (0..n_slots)
                .map(|k| ((step + 1) * (k + 2)) as f64)
                .collect();
            a_data.push(a_row);
            b_data.push(b_row);
        }
        let results = make_arrayed_results(&["A", "B"], &[n_slots, n_slots], &[a_data, b_data]);

        // Both A2A loops coupled (every slot in partition 0), so slot k of
        // each lands in bucket (0, k) -- the streaming helper, called with
        // both loops as members at element k, sums the same two slot-k
        // values into the denominator.
        let partitions =
            mapping_per_slot(&[("A", vec![Some(0); n_slots]), ("B", vec![Some(0); n_slots])]);

        let full = compute_rel_loop_scores_per_element(&results, &partitions);

        for k in 0..n_slots {
            let denom = compute_partition_denominator_for_element(
                &results,
                [("A", n_slots), ("B", n_slots)],
                k,
            );
            let rel_a = compute_rel_loop_score_for_element(&results, "A", n_slots, k, &denom)
                .expect("A must have a series");
            let rel_b = compute_rel_loop_score_for_element(&results, "B", n_slots, k, &denom)
                .expect("B must have a series");

            for step in 0..step_count {
                let full_idx = step * n_slots + k;
                let full_a = full.get("A").unwrap()[full_idx];
                let full_b = full.get("B").unwrap()[full_idx];
                // Bit-for-bit: same arithmetic order, same rounding.
                assert_eq!(
                    rel_a[step], full_a,
                    "loop A step {step} elem {k}: streaming {} vs full {}",
                    rel_a[step], full_a
                );
                assert_eq!(
                    rel_b[step], full_b,
                    "loop B step {step} elem {k}: streaming {} vs full {}",
                    rel_b[step], full_b
                );
            }
        }
    }

    /// Mixed-stride parity: for two coupled A2A loops with different
    /// `n_slots` sharing the same per-slot partition, the streaming pair
    /// (`compute_partition_denominator_for_element` +
    /// `compute_rel_loop_score_for_element`) must produce the same
    /// per-element rel-scores as the full-sweep
    /// `compute_rel_loop_scores_per_element`.  This is the contract the
    /// libsimlin FFI per-partition cache relies on -- the streaming
    /// pair must be a strictly cheaper path to the same numbers.
    ///
    /// Each loop's full-sweep series has its own slot count (A: 3, B: 2);
    /// at slot 2 only A is a member of bucket (0, 2), so the streaming
    /// helper -- called with both loops as members at element 2 but B
    /// gated out by `effective_slot(2, 2) == None` -- agrees.
    #[test]
    fn streaming_helpers_match_full_sweep_in_mixed_stride_partition() {
        // A has n=3, B has n=2, both coupled (every slot in partition 0).
        // Multi-step so we exercise more than one row; distinct-per-step
        // values so any wrong-stride bug shows up loudly.
        //   step 0: A = [1.0, 2.0, 5.0],   B = [10.0, 7.0]
        //   step 1: A = [1.5, 2.5, 6.0],   B = [11.0, 8.0]
        //   step 2: A = [2.0, 3.0, 7.0],   B = [12.0, 9.0]
        let loop_data = vec![
            vec![
                vec![1.0, 2.0, 5.0],
                vec![1.5, 2.5, 6.0],
                vec![2.0, 3.0, 7.0],
            ],
            vec![vec![10.0, 7.0], vec![11.0, 8.0], vec![12.0, 9.0]],
        ];
        let results = make_arrayed_results(&["A", "B"], &[3, 2], &loop_data);

        let partitions = mapping_per_slot(&[
            ("A", vec![Some(0), Some(0), Some(0)]),
            ("B", vec![Some(0), Some(0)]),
        ]);

        let full = compute_rel_loop_scores_per_element(&results, &partitions);
        let full_a = full.get("A").unwrap();
        let full_b = full.get("B").unwrap();
        // A's series is 3-strided, B's is 2-strided.
        assert_eq!(full_a.len(), results.step_count * 3);
        assert_eq!(full_b.len(), results.step_count * 2);

        // The members of bucket (0, k) for k in 0..3 are {A, B} for k<2,
        // {A} for k==2; the streaming helper expresses this via
        // `effective_slot(n_b, k)` returning None for B at k>=n_b.
        for k in 0..3 {
            let denom = compute_partition_denominator_for_element(
                &results,
                [("A", 3_usize), ("B", 2_usize)],
                k,
            );
            let rel_a = compute_rel_loop_score_for_element(&results, "A", 3, k, &denom)
                .expect("A must have a series");
            for step in 0..results.step_count {
                let full_v = full_a[step * 3 + k];
                assert_eq!(
                    rel_a[step], full_v,
                    "loop A step {step} elem {k}: streaming {} vs full {}",
                    rel_a[step], full_v
                );
            }
            if k < 2 {
                let rel_b = compute_rel_loop_score_for_element(&results, "B", 2, k, &denom)
                    .expect("B must have a series");
                for step in 0..results.step_count {
                    let full_v = full_b[step * 2 + k];
                    assert_eq!(
                        rel_b[step], full_v,
                        "loop B step {step} elem {k}: streaming {} vs full {}",
                        rel_b[step], full_v
                    );
                }
            } else {
                // B has no slot k>=2; the streaming helper returns all-zeros
                // for it and the full-sweep series doesn't have that index.
                let rel_b = compute_rel_loop_score_for_element(&results, "B", 2, k, &denom)
                    .expect("B must have a series");
                assert!(rel_b.iter().all(|&v| v == 0.0));
            }
        }
    }

    /// Streaming `compute_partition_denominator_for_element` must skip
    /// arrayed members at partition indices past their own n_slots,
    /// matching the gating now applied by the full-sweep
    /// `compute_rel_loop_scores_per_element`.  Pre-fix the streaming
    /// helper clamped to the loop's last slot via `effective_slot`,
    /// which silently disagreed with the full-sweep helper for any
    /// mixed-stride partition (one arrayed loop with `n_a` slots
    /// sharing a partition with another arrayed loop with `n_b < n_a`).
    /// We plant a sentinel at B's last slot so a clamp would pull
    /// it loudly into the denom; the principled "skip" semantic
    /// excludes B entirely at element 2.
    #[test]
    fn streaming_partition_denominator_skips_arrayed_loops_past_own_slots() {
        // step_count = 1.  A has n=3 with values [1, 2, 5]; B has n=2
        // with values [10, 999.0] (sentinel at slot 1).  At partition
        // element k=2, B has no slot -- the principled denom is
        // |A[2]| = 5, NOT |A[2]| + |B[1]| = 5 + 999 = 1004.
        let loop_data = vec![vec![vec![1.0, 2.0, 5.0]], vec![vec![10.0, 999.0]]];
        let results = make_arrayed_results(&["A", "B"], &[3, 2], &loop_data);

        let denom_at_2 = compute_partition_denominator_for_element(
            &results,
            [("A", 3_usize), ("B", 2_usize)],
            2,
        );
        assert_eq!(
            denom_at_2,
            vec![5.0],
            "B has no slot at partition index 2; its sentinel must NOT \
             pollute the denominator (skip, not clamp)"
        );

        // For sanity, k=0 and k=1 should include both members.
        let denom_at_0 = compute_partition_denominator_for_element(
            &results,
            [("A", 3_usize), ("B", 2_usize)],
            0,
        );
        assert_eq!(denom_at_0, vec![1.0 + 10.0]);
        let denom_at_1 = compute_partition_denominator_for_element(
            &results,
            [("A", 3_usize), ("B", 2_usize)],
            1,
        );
        assert_eq!(denom_at_1, vec![2.0 + 999.0]);
    }

    /// `compute_rel_loop_score_for_element` queried at an element this
    /// loop doesn't have (n=2, queried at k=2) must return all-zeros
    /// rather than clamping to the loop's last slot.  This matches the
    /// full-sweep helper's "zero-fill at positions n..max_slots" rule
    /// so any future caller that directly queries past a loop's range
    /// gets the right answer.
    #[test]
    fn streaming_rel_score_returns_zeros_when_loop_has_no_own_element() {
        // B is arrayed with n=2 and a sentinel value at slot 1.  When
        // queried at element_index=2 it has no own element; the
        // result must be all-zeros, not the slot-1 sentinel rel-score.
        let loop_data = vec![vec![vec![10.0, 999.0]]];
        let results = make_arrayed_results(&["B"], &[2], &loop_data);

        // Use a denom that would produce a clearly-wrong rel-score if
        // the helper clamped: |sentinel|/|denom| would be ~999, but
        // the principled answer is 0.0.
        let denom = vec![1.0_f64];
        let rel = compute_rel_loop_score_for_element(&results, "B", 2, 2, &denom)
            .expect("B has a series");
        assert_eq!(
            rel,
            vec![0.0],
            "queried at index 2 (past B's n=2), result must be 0 not clamped"
        );
    }

    /// SAFEDIV-0 semantics propagate per element: a partition where every
    /// member's queried slot is identically zero must yield 0 (not NaN)
    /// for the rel-score, matching the scalar-helper contract.
    #[test]
    fn per_element_streaming_safediv_zero() {
        // Both loops have slot 0 = 0 across all steps but slot 1 != 0;
        // querying element 0 must yield 0 (no panic, no NaN).
        let loop_data = vec![
            vec![vec![0.0, 5.0], vec![0.0, 4.0]],
            vec![vec![0.0, 3.0], vec![0.0, 2.0]],
        ];
        let results = make_arrayed_results(&["A", "B"], &[2, 2], &loop_data);

        let denom = compute_partition_denominator_for_element(
            &results,
            [("A", 2_usize), ("B", 2_usize)],
            0,
        );
        assert_eq!(denom, vec![0.0, 0.0]);

        let rel = compute_rel_loop_score_for_element(&results, "A", 2, 0, &denom).unwrap();
        for v in rel {
            assert_eq!(v, 0.0, "SAFEDIV-0 must yield 0, got {v}");
        }
    }

    /// The streaming FFI denominator (`compute_partition_denominator_for_element`)
    /// must exclude a `NaN` summand and keep an `Inf` one, exactly like the
    /// full-sweep helper -- this is the path libsimlin's
    /// `simlin_analyze_get_relative_loop_score` cache drives, so the
    /// GH #542 fix must hold there too.
    #[test]
    fn per_element_streaming_denominator_excludes_nan_keeps_inf() {
        // step 0: A slot0 = NaN, B slot0 = 3  -> denom = 3 (NaN dropped).
        // step 1: A slot0 = Inf, B slot0 = 3  -> denom = Inf (Inf kept).
        let loop_data = vec![
            vec![vec![f64::NAN, 7.0], vec![f64::INFINITY, 7.0]],
            vec![vec![3.0, 1.0], vec![3.0, 1.0]],
        ];
        let results = make_arrayed_results(&["A", "B"], &[2, 2], &loop_data);

        let denom = compute_partition_denominator_for_element(
            &results,
            [("A", 2_usize), ("B", 2_usize)],
            0,
        );
        assert_eq!(denom[0], 3.0, "NaN summand excluded from streaming denom");
        assert_eq!(
            denom[1],
            f64::INFINITY,
            "Inf summand retained in streaming denom"
        );

        // The healthy sibling B normalizes against the NaN-free denom at
        // step 0 (3/3 = 1) and goes to 0 against the +Inf denom at step 1.
        let rel_b = compute_rel_loop_score_for_element(&results, "B", 2, 0, &denom).unwrap();
        assert!(
            (rel_b[0] - 1.0).abs() < 1e-12,
            "healthy B not poisoned: {}",
            rel_b[0]
        );
        assert_eq!(rel_b[1], 0.0, "B dominated by +Inf sibling -> 0");
    }

    /// An absent loop_score variable returns `None`, matching the
    /// `compute_rel_loop_score_for_id` contract.
    #[test]
    fn per_element_streaming_absent_loop_returns_none() {
        let results = make_arrayed_results(&["A"], &[2], &[vec![vec![1.0, 2.0], vec![3.0, 4.0]]]);
        let denom = compute_partition_denominator_for_element(&results, [("A", 2_usize)], 0);
        assert!(compute_rel_loop_score_for_element(&results, "missing", 2, 0, &denom).is_none());
    }

    /// Signed argmax-abs aggregator: at each step, return the signed
    /// rel-score of the element with the largest `|rel[k, t]|`.  The
    /// dominant element can switch between steps; the sign is preserved
    /// from whichever element won that step.
    #[test]
    fn argmax_abs_picks_dominant_element_with_sign() {
        // 2 elements, 2 steps.  loop_score chosen so element 0 dominates
        // at step 0 and element 1 dominates at step 1, with opposite signs.
        //   step 0: slot 0 =  5,  slot 1 = -1   -> rel = 0.5, -0.1
        //   step 1: slot 0 =  1,  slot 1 = -8   -> rel = 0.1, -0.8
        let loop_data = vec![vec![vec![5.0, -1.0], vec![1.0, -8.0]]];
        let results = make_arrayed_results(&["L"], &[2], &loop_data);

        // Constant denom of 10 per step per element.
        let denoms = vec![vec![10.0_f64; 2]; 2];
        let denom_refs: Vec<&[f64]> = denoms.iter().map(|d| d.as_slice()).collect();

        let agg = compute_rel_loop_score_argmax_abs(&results, "L", 2, &denom_refs)
            .expect("L must have a series");

        // step 0: argmax-abs is slot 0 (|0.5| > |-0.1|), signed value = +0.5.
        // step 1: argmax-abs is slot 1 (|-0.8| > |0.1|), signed value = -0.8.
        assert_eq!(agg, vec![0.5, -0.8]);
    }

    /// Scalar (n_slots == 1) reduces to identity: the aggregator returns
    /// the same series as `compute_rel_loop_score_for_id` would.
    #[test]
    fn argmax_abs_scalar_reduces_to_identity() {
        let loop_data = vec![vec![vec![3.0], vec![-7.0]]];
        let results = make_arrayed_results(&["L"], &[1], &loop_data);
        let denoms = [vec![10.0_f64, 10.0]];
        let denom_refs: Vec<&[f64]> = denoms.iter().map(|d| d.as_slice()).collect();

        let agg = compute_rel_loop_score_argmax_abs(&results, "L", 1, &denom_refs)
            .expect("L must have a series");
        assert_eq!(agg, vec![0.3, -0.7]);
    }

    /// Ties (two elements with equal |rel|) are broken deterministically:
    /// the lowest slot index wins.  This matches Rust's stable
    /// `Ord`/`max_by_key` convention for "first hit on equal".
    #[test]
    fn argmax_abs_ties_broken_by_lowest_index() {
        // Both slots have equal magnitude at step 0; slot 0 has positive
        // sign and slot 1 has negative sign.  The output must be slot 0's
        // value (+0.4), not slot 1's.
        let loop_data = vec![vec![vec![4.0, -4.0]]];
        let results = make_arrayed_results(&["L"], &[2], &loop_data);
        let denoms = [vec![10.0_f64], vec![10.0_f64]];
        let denom_refs: Vec<&[f64]> = denoms.iter().map(|d| d.as_slice()).collect();

        let agg = compute_rel_loop_score_argmax_abs(&results, "L", 2, &denom_refs)
            .expect("L must have a series");
        assert_eq!(agg, vec![0.4]);
    }

    /// Absent loop returns `None`, matching the streaming-helper contract.
    #[test]
    fn argmax_abs_absent_loop_returns_none() {
        let results = make_arrayed_results(&["L"], &[2], &[vec![vec![1.0, 2.0]]]);
        let denoms = [vec![10.0_f64], vec![10.0]];
        let denom_refs: Vec<&[f64]> = denoms.iter().map(|d| d.as_slice()).collect();
        assert!(compute_rel_loop_score_argmax_abs(&results, "missing", 2, &denom_refs).is_none());
    }

    /// `LoopElementIndex` for a scalar loop reports empty dimensions
    /// and `n_slots = 1`.  Used by the libsimlin FFI dispatch to detect
    /// "this loop is not arrayed, reject subscripted IDs."
    #[test]
    fn loop_element_index_scalar_loop() {
        let ltm_vars = vec![crate::db::LtmSyntheticVar {
            name: "$\u{205A}ltm\u{205A}loop_score\u{205A}r1".to_string(),
            equation: crate::datamodel::Equation::Scalar("1.0".to_string()),
            dimensions: vec![],
            compile_directly: false,
        }];
        let project_dims: Vec<crate::datamodel::Dimension> = vec![];

        let index = build_loop_element_index(&ltm_vars, &project_dims);
        let entry = index.get("r1").expect("r1 should be indexed");
        assert!(entry.dimensions.is_empty());
        assert!(entry.dim_elements.is_empty());
        assert!(entry.is_indexed.is_empty());
        assert!(entry.dim_sizes.is_empty());
        assert_eq!(entry.n_slots, 1);
    }

    /// 1D named-dim loop indexes one dimension.  Element names are stored
    /// in canonical form (lowercased) so the FFI subscript resolver can
    /// compare against canonicalized user input directly.
    #[test]
    fn loop_element_index_named_1d() {
        let ltm_vars = vec![crate::db::LtmSyntheticVar {
            name: "$\u{205A}ltm\u{205A}loop_score\u{205A}r1".to_string(),
            equation: crate::datamodel::Equation::Scalar("1.0".to_string()),
            dimensions: vec!["Region".to_string()],
            compile_directly: false,
        }];
        let project_dims = vec![crate::datamodel::Dimension::named(
            "Region".to_string(),
            vec!["NYC".to_string(), "Boston".to_string(), "LA".to_string()],
        )];

        let index = build_loop_element_index(&ltm_vars, &project_dims);
        let entry = index.get("r1").expect("r1 should be indexed");
        assert_eq!(entry.dimensions, vec!["region".to_string()]);
        assert_eq!(entry.is_indexed, vec![false]);
        assert_eq!(entry.dim_sizes, vec![3]);
        assert_eq!(entry.n_slots, 3);
        assert_eq!(entry.dim_elements.len(), 1);
        assert_eq!(
            entry.dim_elements[0],
            vec!["nyc".to_string(), "boston".to_string(), "la".to_string()]
        );
    }

    /// 2D mixed (named × indexed) loop preserves declaration order in
    /// `dimensions`, `is_indexed`, and `dim_sizes`.  For indexed dims,
    /// `dim_elements` is empty -- the resolver parses the subscript as
    /// a 1-based integer rather than matching against names.
    #[test]
    fn loop_element_index_mixed_2d() {
        let ltm_vars = vec![crate::db::LtmSyntheticVar {
            name: "$\u{205A}ltm\u{205A}loop_score\u{205A}r1".to_string(),
            equation: crate::datamodel::Equation::Scalar("1.0".to_string()),
            dimensions: vec!["Region".to_string(), "Cohort".to_string()],
            compile_directly: false,
        }];
        let project_dims = vec![
            crate::datamodel::Dimension::named(
                "Region".to_string(),
                vec!["NYC".to_string(), "Boston".to_string()],
            ),
            crate::datamodel::Dimension::indexed("Cohort".to_string(), 4),
        ];

        let index = build_loop_element_index(&ltm_vars, &project_dims);
        let entry = index.get("r1").expect("r1 should be indexed");
        assert_eq!(
            entry.dimensions,
            vec!["region".to_string(), "cohort".to_string()]
        );
        assert_eq!(entry.is_indexed, vec![false, true]);
        assert_eq!(entry.dim_sizes, vec![2, 4]);
        assert_eq!(entry.n_slots, 8);
        assert_eq!(
            entry.dim_elements[0],
            vec!["nyc".to_string(), "boston".to_string()]
        );
        // Indexed dims have no element-name list; the resolver parses the
        // subscript as a 1..=size integer instead.
        assert!(entry.dim_elements[1].is_empty());
    }

    /// Resolver: 1D named dim with canonical-element matching.  Element
    /// names are case-insensitive thanks to internal canonicalize.
    #[test]
    fn resolve_1d_named() {
        let ltm_vars = vec![crate::db::LtmSyntheticVar {
            name: "$\u{205A}ltm\u{205A}loop_score\u{205A}r1".to_string(),
            equation: crate::datamodel::Equation::Scalar("1.0".to_string()),
            dimensions: vec!["Region".to_string()],
            compile_directly: false,
        }];
        let project_dims = vec![crate::datamodel::Dimension::named(
            "Region".to_string(),
            vec!["NYC".to_string(), "Boston".to_string(), "LA".to_string()],
        )];
        let index = build_loop_element_index(&ltm_vars, &project_dims);
        let r1 = index.get("r1").unwrap();
        assert_eq!(r1.resolve(&["NYC"]).unwrap(), 0);
        assert_eq!(r1.resolve(&["Boston"]).unwrap(), 1);
        assert_eq!(r1.resolve(&["LA"]).unwrap(), 2);
        // Case-insensitive: canonicalize lowercases.
        assert_eq!(r1.resolve(&["BOSTON"]).unwrap(), 1);
        assert_eq!(r1.resolve(&["boston"]).unwrap(), 1);
    }

    /// Resolver: 2D mixed (named × indexed) with row-major linear offset.
    /// Strides are [s_1, 1] for [d_0, d_1], so linear = i_0*s_1 + i_1.
    #[test]
    fn resolve_2d_named_indexed_row_major() {
        let ltm_vars = vec![crate::db::LtmSyntheticVar {
            name: "$\u{205A}ltm\u{205A}loop_score\u{205A}r1".to_string(),
            equation: crate::datamodel::Equation::Scalar("1.0".to_string()),
            dimensions: vec!["Region".to_string(), "Cohort".to_string()],
            compile_directly: false,
        }];
        let project_dims = vec![
            crate::datamodel::Dimension::named(
                "Region".to_string(),
                vec!["NYC".to_string(), "Boston".to_string()],
            ),
            crate::datamodel::Dimension::indexed("Cohort".to_string(), 4),
        ];
        let index = build_loop_element_index(&ltm_vars, &project_dims);
        let r1 = index.get("r1").unwrap();
        // [NYC=0, Cohort=1] -> 0*4 + 0 = 0
        assert_eq!(r1.resolve(&["NYC", "1"]).unwrap(), 0);
        // [NYC=0, Cohort=4] -> 0*4 + 3 = 3
        assert_eq!(r1.resolve(&["NYC", "4"]).unwrap(), 3);
        // [Boston=1, Cohort=1] -> 1*4 + 0 = 4
        assert_eq!(r1.resolve(&["Boston", "1"]).unwrap(), 4);
        // [Boston=1, Cohort=3] -> 1*4 + 2 = 6
        assert_eq!(r1.resolve(&["Boston", "3"]).unwrap(), 6);
        // [Boston=1, Cohort=4] -> 1*4 + 3 = 7 (last slot)
        assert_eq!(r1.resolve(&["Boston", "4"]).unwrap(), 7);
    }

    /// Resolver: scalar loop with no subscripts returns offset 0.  Scalar
    /// loops with explicit subscripts error -- they can't be subscripted.
    #[test]
    fn resolve_scalar_loop() {
        let ltm_vars = vec![crate::db::LtmSyntheticVar {
            name: "$\u{205A}ltm\u{205A}loop_score\u{205A}r1".to_string(),
            equation: crate::datamodel::Equation::Scalar("1.0".to_string()),
            dimensions: vec![],
            compile_directly: false,
        }];
        let project_dims: Vec<crate::datamodel::Dimension> = vec![];
        let index = build_loop_element_index(&ltm_vars, &project_dims);
        let r1 = index.get("r1").unwrap();
        // Bare access on scalar loop: offset 0.
        assert_eq!(r1.resolve(&[]).unwrap(), 0);
        // Subscripts on scalar loop: error.
        assert!(matches!(
            r1.resolve(&["Boston"]),
            Err(ResolveError::DimCountMismatch {
                expected: 0,
                got: 1
            })
        ));
    }

    /// Resolver error: dim count mismatch.
    #[test]
    fn resolve_dim_count_mismatch() {
        let ltm_vars = vec![crate::db::LtmSyntheticVar {
            name: "$\u{205A}ltm\u{205A}loop_score\u{205A}r1".to_string(),
            equation: crate::datamodel::Equation::Scalar("1.0".to_string()),
            dimensions: vec!["Region".to_string()],
            compile_directly: false,
        }];
        let project_dims = vec![crate::datamodel::Dimension::named(
            "Region".to_string(),
            vec!["NYC".to_string(), "Boston".to_string()],
        )];
        let index = build_loop_element_index(&ltm_vars, &project_dims);
        let r1 = index.get("r1").unwrap();
        // Empty subscripts on a 1D loop is an error.
        assert!(matches!(
            r1.resolve(&[]),
            Err(ResolveError::DimCountMismatch {
                expected: 1,
                got: 0
            })
        ));
        // Two subscripts on a 1D loop is an error.
        assert!(matches!(
            r1.resolve(&["NYC", "extra"]),
            Err(ResolveError::DimCountMismatch {
                expected: 1,
                got: 2
            })
        ));
    }

    /// Resolver error: unknown named element.
    #[test]
    fn resolve_unknown_element() {
        let ltm_vars = vec![crate::db::LtmSyntheticVar {
            name: "$\u{205A}ltm\u{205A}loop_score\u{205A}r1".to_string(),
            equation: crate::datamodel::Equation::Scalar("1.0".to_string()),
            dimensions: vec!["Region".to_string()],
            compile_directly: false,
        }];
        let project_dims = vec![crate::datamodel::Dimension::named(
            "Region".to_string(),
            vec!["NYC".to_string(), "Boston".to_string()],
        )];
        let index = build_loop_element_index(&ltm_vars, &project_dims);
        let r1 = index.get("r1").unwrap();
        match r1.resolve(&["Tokyo"]) {
            Err(ResolveError::ElementNotFound { dim, value }) => {
                assert_eq!(dim, "region");
                assert_eq!(value, "tokyo");
            }
            other => panic!("expected ElementNotFound, got {:?}", other),
        }
    }

    /// Resolver error: indexed-dim subscript out of range or non-numeric.
    #[test]
    fn resolve_indexed_errors() {
        let ltm_vars = vec![crate::db::LtmSyntheticVar {
            name: "$\u{205A}ltm\u{205A}loop_score\u{205A}r1".to_string(),
            equation: crate::datamodel::Equation::Scalar("1.0".to_string()),
            dimensions: vec!["Cohort".to_string()],
            compile_directly: false,
        }];
        let project_dims = vec![crate::datamodel::Dimension::indexed(
            "Cohort".to_string(),
            3,
        )];
        let index = build_loop_element_index(&ltm_vars, &project_dims);
        let r1 = index.get("r1").unwrap();
        // Out of range: indexed dim is 1..=3.
        assert!(matches!(
            r1.resolve(&["0"]),
            Err(ResolveError::IndexOutOfRange { .. })
        ));
        assert!(matches!(
            r1.resolve(&["4"]),
            Err(ResolveError::IndexOutOfRange { .. })
        ));
        // Non-integer subscript on indexed dim.
        assert!(matches!(
            r1.resolve(&["foo"]),
            Err(ResolveError::InvalidIntegerSubscript { .. })
        ));
    }

    /// Non-loop_score LTM vars (link_score, path, composite) are filtered
    /// out -- the index is keyed only by detected loop IDs.
    #[test]
    fn loop_element_index_filters_non_loop_score_vars() {
        let ltm_vars = vec![
            crate::db::LtmSyntheticVar {
                name: "$\u{205A}ltm\u{205A}link_score\u{205A}a\u{2192}b".to_string(),
                equation: crate::datamodel::Equation::Scalar("1.0".to_string()),
                dimensions: vec![],
                compile_directly: false,
            },
            crate::db::LtmSyntheticVar {
                name: "$\u{205A}ltm\u{205A}loop_score\u{205A}r1".to_string(),
                equation: crate::datamodel::Equation::Scalar("1.0".to_string()),
                dimensions: vec![],
                compile_directly: false,
            },
            crate::db::LtmSyntheticVar {
                name: "$\u{205A}ltm\u{205A}path\u{205A}foo\u{205A}0".to_string(),
                equation: crate::datamodel::Equation::Scalar("1.0".to_string()),
                dimensions: vec![],
                compile_directly: false,
            },
        ];
        let project_dims: Vec<crate::datamodel::Dimension> = vec![];
        let index = build_loop_element_index(&ltm_vars, &project_dims);
        assert_eq!(index.len(), 1);
        assert!(index.contains_key("r1"));
    }

    /// SAFEDIV-0 propagates per element: an element with zero denom at a
    /// given step contributes a 0 rel-score for that element at that step.
    /// If every element's denom is zero, the aggregator returns 0.
    #[test]
    fn argmax_abs_safediv_zero_per_element() {
        // step 0: both elements have denom 0 -> both rel = 0 -> agg = 0.
        // step 1: element 0 has denom 0 (rel=0); element 1 has denom 10
        //         and slot value -7 -> rel = -0.7, dominant.
        let loop_data = vec![vec![vec![5.0, 5.0], vec![5.0, -7.0]]];
        let results = make_arrayed_results(&["L"], &[2], &loop_data);
        let denoms = [
            vec![0.0_f64, 0.0],  // element 0
            vec![0.0_f64, 10.0], // element 1
        ];
        let denom_refs: Vec<&[f64]> = denoms.iter().map(|d| d.as_slice()).collect();

        let agg = compute_rel_loop_score_argmax_abs(&results, "L", 2, &denom_refs)
            .expect("L must have a series");
        assert_eq!(agg.len(), 2);
        // step 0: all elements safe-div to 0 -> agg = 0.
        assert_eq!(agg[0], 0.0);
        // step 1: only element 1 is non-zero, signed value -0.7.
        assert!((agg[1] - (-0.7)).abs() < 1e-12, "got {}", agg[1]);
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
            // Scalar-loop shape: one slot per loop (the slot-0 convenience
            // view ignores anything beyond slot 0 anyway).
            let loop_partitions: HashMap<String, Vec<Option<usize>>> = loop_ids
                .iter()
                .enumerate()
                .map(|(i, id)| (id.clone(), vec![Some(raw_partitions[i] % num_partitions)]))
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

        /// `compute_rel_loop_scores_per_element` must match the naive
        /// per-`(partition, slot)` reference for arbitrary multi-slot
        /// partition vectors -- coupled (all entries the same `Some(p)`),
        /// uncoupled (distinct `Some(p)` per slot), `None`-laced, and
        /// scalar.  This is the regression net for the GH #487 bucket
        /// grouping: the optimized BTreeMap-grid implementation and the
        /// scan-all-loops reference are computed independently, so any
        /// divergence in the broadcast stride, the slot gating, or the
        /// SAFEDIV-0 handling shows up here.
        ///
        /// Per-loop `spec[i] = (kind, len, base, vals)` builds the
        /// partition vector:
        ///   - kind 0: scalar `[Some(base)]`.
        ///   - kind 1: scalar `[None]`.
        ///   - kind 2: coupled arrayed `[Some(base); len]`.
        ///   - kind 3: uncoupled arrayed `[Some(base), Some(base+1), ...]`
        ///     (distinct consecutive partitions).
        ///   - kind 4: `None`-laced arrayed -- `vals[k]` chooses
        ///     `Some(vals[k])` or `None` per slot.
        /// Partition indices stay in a small pool (so coupling across
        /// *different* loops actually happens); lengths stay tiny so 128
        /// cases run in well under a second on a debug build.
        #[test]
        fn per_element_matches_naive_reference(
            specs in prop::collection::vec(
                (
                    0usize..=4,                            // kind
                    1usize..=3,                            // arrayed length
                    0usize..=3,                            // base partition
                    prop::collection::vec(0usize..=4, 3), // per-slot None/Some chooser (>=4 => None)
                ),
                1..=4,
            ),
            num_steps in 1usize..=4,
            // Flat pool of loop_score samples; sliced per (loop, slot, step).
            // Includes 0.0 so the SAFEDIV-0 path is exercised.
            raw_vals in prop::collection::vec(-50.0_f64..=50.0_f64, 1..=300),
        ) {
            let n = specs.len();
            let loop_ids: Vec<String> = (0..n).map(|i| format!("L{i}")).collect();

            // Materialize each loop's per-slot partition vector.
            let slots: Vec<Vec<Option<usize>>> = specs
                .iter()
                .map(|(kind, len, base, vals)| match kind {
                    0 => vec![Some(*base)],
                    1 => vec![None],
                    2 => vec![Some(*base); *len],
                    3 => (0..*len).map(|k| Some(*base + k)).collect(),
                    _ => (0..*len)
                        .map(|k| {
                            let v = vals[k % vals.len()];
                            if v >= 4 { None } else { Some(v) }
                        })
                        .collect(),
                })
                .collect();
            let n_slots: Vec<usize> = slots.iter().map(|v| v.len().max(1)).collect();

            // Build per-(loop, step, slot) loop_score data from the flat
            // pool, advancing a single cursor so successive slots get
            // distinct samples.  `series[i][step][slot]`.
            let mut cursor = 0usize;
            let mut series: Vec<Vec<Vec<f64>>> = Vec::with_capacity(n);
            for &ns in &n_slots {
                let mut per_step = Vec::with_capacity(num_steps);
                for _ in 0..num_steps {
                    let mut per_slot = Vec::with_capacity(ns);
                    for _ in 0..ns {
                        per_slot.push(raw_vals[cursor % raw_vals.len()]);
                        cursor += 1;
                    }
                    per_step.push(per_slot);
                }
                series.push(per_step);
            }

            let results = make_arrayed_results(
                &loop_ids.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                &n_slots,
                &series,
            );
            let loop_partitions: HashMap<String, Vec<Option<usize>>> = loop_ids
                .iter()
                .zip(slots.iter())
                .map(|(id, v)| (id.clone(), v.clone()))
                .collect();

            let actual = compute_rel_loop_scores_per_element(&results, &loop_partitions);
            let expected =
                reference_rel_loop_scores_per_element(&loop_ids, &slots, &series, num_steps);

            for (i, id) in loop_ids.iter().enumerate() {
                let a = actual.get(id).expect("every loop has a series");
                let e = &expected[i];
                prop_assert_eq!(
                    a.len(),
                    e.len(),
                    "loop {}: series length {} vs reference {}",
                    id,
                    a.len(),
                    e.len()
                );
                for (idx, (&av, &ev)) in a.iter().zip(e.iter()).enumerate() {
                    if av.is_nan() && ev.is_nan() {
                        continue;
                    }
                    // The two paths sum the same floats in the same order
                    // (sorted-loop-id member lists; ids are `L{i}`, i < 4),
                    // so the result is bit-identical, not merely close.
                    prop_assert_eq!(
                        av, ev,
                        "loop {} flat-index {}: actual {} vs reference {}", id, idx, av, ev
                    );
                }
            }
        }
    }
}
