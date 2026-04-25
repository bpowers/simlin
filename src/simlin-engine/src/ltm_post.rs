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

/// Resolve the slot offset to read for a loop with `n_slots` slots when
/// the partition is being queried at `element_index`.
///
/// - Scalar loops (`n_slots <= 1`) always read slot 0 (broadcast).
/// - Arrayed loops read slot `element_index`, saturating at `n_slots - 1`
///   if the queried element is past the loop's slot count.
///
/// Saturation matches the partition-wide max-slots stride convention used
/// by [`compute_rel_loop_scores_per_element`]: when partitions mix loops
/// with different per-loop slot counts, the smaller-N loops are read at
/// their last slot rather than producing UB through out-of-range access.
/// Callers that have already validated `element_index < n_slots` (e.g.
/// the FFI subscript resolver) pay no runtime cost from this clamp.
fn effective_slot(n_slots: usize, element_index: usize) -> usize {
    if n_slots <= 1 {
        0
    } else if element_index >= n_slots {
        n_slots - 1
    } else {
        element_index
    }
}

/// Per-element streaming variant of [`compute_partition_denominator`].
///
/// For each `(loop_id, n_slots)` in the iterator whose `loop_score`
/// variable is present in `results`, contributes
/// `|row[off + effective_slot(n_slots, element_index)]|` to the partition
/// sum at every step.  Scalar loops (`n_slots <= 1`) broadcast slot 0;
/// arrayed loops with fewer slots than the queried element clamp to the
/// last slot.
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
            results
                .offsets
                .get(&loop_score_ident(id))
                .copied()
                .map(|off| (off, effective_slot(n_slots, element_index)))
        })
        .collect();

    let mut denom = vec![0.0_f64; results.step_count];
    for (t, row) in results.iter().enumerate() {
        denom[t] = entries
            .iter()
            .map(|&(off, slot)| row[off + slot].abs())
            .sum();
    }
    denom
}

/// Per-element streaming variant of [`compute_rel_loop_score_for_id`].
///
/// Reads `row[off + effective_slot(n_slots, element_index)]` as the
/// numerator at each step, paired with the per-element partition
/// denominator from [`compute_partition_denominator_for_element`].
/// SAFEDIV-0, NaN-propagation, and "absent loop returns `None`"
/// semantics match the scalar streaming helper.
pub fn compute_rel_loop_score_for_element(
    results: &Results,
    loop_id: &str,
    n_slots: usize,
    element_index: usize,
    denominator: &[f64],
) -> Option<Vec<f64>> {
    let off = results.offsets.get(&loop_score_ident(loop_id)).copied()?;
    let slot = effective_slot(n_slots, element_index);
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
            let slot = effective_slot(n_slots, k);
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

        let partitions = mapping(&[("A", Some(0)), ("B", Some(0))]);
        let mut slots = HashMap::new();
        slots.insert("A".to_string(), n_slots);
        slots.insert("B".to_string(), n_slots);

        let full = compute_rel_loop_scores_per_element(&results, &partitions, &slots);

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
            equation: "1.0".to_string(),
            dimensions: vec![],
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
            equation: "1.0".to_string(),
            dimensions: vec!["Region".to_string()],
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
            equation: "1.0".to_string(),
            dimensions: vec!["Region".to_string(), "Cohort".to_string()],
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
            equation: "1.0".to_string(),
            dimensions: vec!["Region".to_string()],
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
            equation: "1.0".to_string(),
            dimensions: vec!["Region".to_string(), "Cohort".to_string()],
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
            equation: "1.0".to_string(),
            dimensions: vec![],
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
            equation: "1.0".to_string(),
            dimensions: vec!["Region".to_string()],
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
            equation: "1.0".to_string(),
            dimensions: vec!["Region".to_string()],
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
            equation: "1.0".to_string(),
            dimensions: vec!["Cohort".to_string()],
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
                equation: "1.0".to_string(),
                dimensions: vec![],
            },
            crate::db::LtmSyntheticVar {
                name: "$\u{205A}ltm\u{205A}loop_score\u{205A}r1".to_string(),
                equation: "1.0".to_string(),
                dimensions: vec![],
            },
            crate::db::LtmSyntheticVar {
                name: "$\u{205A}ltm\u{205A}path\u{205A}foo\u{205A}0".to_string(),
                equation: "1.0".to_string(),
                dimensions: vec![],
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
