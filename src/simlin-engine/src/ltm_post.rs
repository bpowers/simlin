// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Post-simulation relative loop scores: the one owner of the cycle-partition
//! normalization every surface reads.
//!
//! A loop's raw `loop_score` is the product of its link scores; its
//! *relative* score at a step is that score divided by the sum of `|score|`
//! over every loop of the same cycle partition (reference section 4.4).
//! Exhaustive mode records the raw series in [`Results`] (one column per
//! slot of the `$⁚ltm⁚loop_score⁚{id}` synthetic); discovery holds them on
//! its `FoundLoop`s.  Both feed [`group_totals`] and [`relative_series`], so
//! the two surfaces cannot disagree on what "relative" means, and every
//! reader of the exhaustive series -- the libsimlin FFI, the layout's
//! importance series, the tests -- goes through [`compute_rel_loop_scores`]
//! rather than dividing on its own.
//!
//! Normalization is per partition and nothing finer.  Every `(loop, slot)`
//! whose stocks resolve to partition `p` is a member of `p`'s group: a scalar
//! loop is one member, an arrayed loop one member per element slot.  This is
//! the de-subscripted reading the reference's section 15.4 promises -- an
//! arrayed loop's element `k` competes with its sibling elements and with
//! every scalar loop of the partition exactly as the N scalar loops of the
//! hand-expanded model would.  Never key the group on the slot index as
//! well: a slot index is a position in one loop's own dimension space, so
//! two arrayed loops over different dimension lists (or a scalar loop, which
//! has no elements) attach no shared meaning to "slot k", and grouping by it
//! splits one partition into denominators that each miss most of its
//! members: on `test/cross_element_ltm` a per-slot key reads the births
//! loop's share as 1/2 at one element and 2/3 at the other, where the
//! partition holds four active members and the share is 1/3 at both.
//!
//! The relative scores are computed here rather than emitted as synthetic
//! VM variables: a `rel_loop_score` equation names every sibling's
//! `loop_score`, O(P²) text per partition, which dominated compile memory on
//! dense models (`docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md`).
//! Post-simulation the cost is O(P × save_steps) over series the VM writes
//! anyway.

use std::collections::HashMap;
use std::hash::Hash;

use indexmap::IndexMap;

use crate::common::{Canonical, Ident};
use crate::results::Results;

/// The normalization group one member of a relative-score computation
/// belongs to (GH #750).
///
/// A member whose cycle partition resolves is in that
/// [`NormGroup::Partition`] and normalizes against every other member of
/// it.  A member whose partition is unresolved (`None` -- a loop genuinely
/// below the parent stock graph: module-internal-stock loops,
/// PREVIOUS-lagged stockless loops) gets a [`NormGroup::Solo`] group of its
/// own, so two UNRELATED unpartitioned members can never share a
/// denominator -- the GH #487-class cross-pollution a shared default `None`
/// bucket reintroduces.  A solo member's relative score collapses to the
/// lone-pin degeneracy documented on [`compute_rel_loop_scores`]:
/// sign-preserving `+/-1` when active, `0` via SAFEDIV-0 when not.  Loops
/// genuinely coupled through module-internal state are under-merged by this
/// rule -- each normalizes alone instead of against its true siblings;
/// resolving partitions for state-carrying module instances is the
/// refinement that would move those loops out of the `None` bucket.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) enum NormGroup {
    /// A resolved cycle partition (the engine-internal partition index).
    Partition(usize),
    /// An unresolved-partition member, alone in its own group.  The payload
    /// is the member's index in the caller's member list -- a `(loop, slot)`
    /// in exhaustive mode, a discovered loop in discovery -- which is unique
    /// per member; the value itself is never interpreted.
    Solo(usize),
}

impl NormGroup {
    /// The group of the member at `member_idx` whose cycle partition is
    /// `partition`.
    pub(crate) fn for_member(partition: Option<usize>, member_idx: usize) -> NormGroup {
        match partition {
            Some(p) => NormGroup::Partition(p),
            None => NormGroup::Solo(member_idx),
        }
    }
}

/// Add one member's `|score|` at a step to its group's running total.
///
/// A `NaN` score means that member's score is *undefined* at the step; it
/// is not signal, so it contributes nothing -- otherwise a single bad member
/// turns the whole group's denominator into `NaN` (the `total == 0.0`
/// SAFEDIV guard does not fire on `NaN`), poisoning every sibling's relative
/// score (GH #542).  The bad member's own numerator stays `NaN`, so as long
/// as a healthy sibling keeps the total non-zero its own relative score
/// stays `NaN` -- the honest per-member "undefined here" signal.  (When the
/// `NaN` member is the group's only contributor the total stays `0.0` and
/// SAFEDIV-0 yields `0.0` instead.)  This is also the discovery rule: a
/// `NaN` link score marks its edge inactive and contributes nothing.
///
/// `+/-Inf` is deliberately kept: a raw loop score legitimately diverges at
/// a dominance inflection (the link-score denominators go to zero there),
/// so an `Inf` summand is real signal that the member dominates.  Keeping
/// it sends the dominated siblings to `0` (`finite/Inf`) and the dominant
/// member to `NaN` (`Inf/Inf`).  A FINITE sum that overflows saturates to
/// `f64::MAX` instead of becoming `Inf`: every summand was finite, so an
/// infinite total would zero every finite share for no member's benefit.
#[inline]
pub(crate) fn add_to_total(total: &mut f64, score: f64) {
    if score.is_nan() {
        return;
    }
    let mass = score.abs();
    let sum = *total + mass;
    *total = if sum.is_infinite() && mass.is_finite() && total.is_finite() {
        f64::MAX
    } else {
        sum
    };
}

/// Per-group, per-step `Σ|score|` over `members`: the denominator of every
/// relative score.
///
/// `members` yields `(group, scores)` pairs; each member's `|score|` is
/// added to its group's total at every step through `add_to_total` (`NaN`
/// excluded, `Inf` kept, finite overflow saturating).  Members are
/// accumulated in the order given, and IEEE-754 addition is not
/// associative, so callers pass them in a deterministic order -- the
/// exhaustive path's loop emission order (GH #468), the discovery path's
/// candidate order.  A series shorter than `step_count` contributes nothing
/// past its end.
///
/// The key type is generic so the loop-level normalization (keyed by
/// [`NormGroup`]) and the link-level one (keyed by the link's target) share
/// this single accumulator rather than each restating the summand rule.
pub fn group_totals<K, I, S>(members: I, step_count: usize) -> HashMap<K, Vec<f64>>
where
    K: Hash + Eq,
    I: IntoIterator<Item = (K, S)>,
    S: IntoIterator<Item = f64>,
{
    let mut totals: HashMap<K, Vec<f64>> = HashMap::new();
    for (group, scores) in members {
        let total = totals
            .entry(group)
            .or_insert_with(|| vec![0.0_f64; step_count]);
        for (t, score) in scores.into_iter().take(step_count).enumerate() {
            add_to_total(&mut total[t], score);
        }
    }
    totals
}

/// `score[t] / totals[t]` at every step, with SAFEDIV-0 semantics: a `0`
/// total yields `0` rather than `NaN`.  A `NaN` numerator propagates (the
/// member is undefined at that step); `Inf/Inf` is `NaN` at a dominance
/// inflection.  A step past the end of `totals` reads a `0` total.
pub fn relative_series<S>(scores: S, totals: &[f64]) -> Vec<f64>
where
    S: IntoIterator<Item = f64>,
{
    scores
        .into_iter()
        .enumerate()
        .map(|(t, score)| {
            let total = totals.get(t).copied().unwrap_or(0.0);
            if total == 0.0 { 0.0 } else { score / total }
        })
        .collect()
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

/// Per-loop, per-slot, per-step relative loop scores from an exhaustive-mode
/// [`Results`] -- the owner every reader of those scores goes through.
///
/// `loop_partitions` maps each loop id to its **per-slot** cycle-partition
/// vector as `model_ltm_variables` produces it: length 1 for a scalar,
/// cross-element or mixed loop, one entry per element for an A2A loop, in
/// the row-major slot order the `loop_score` columns use.  Every `(loop,
/// slot)` with a `loop_score` column is one member of the group
/// [`NormGroup::for_member`] assigns to its slot's partition, and its
/// relative score is [`relative_series`] against that group's
/// [`group_totals`].  A scalar loop therefore has exactly one series and an
/// arrayed loop one per slot, each normalized against every member of the
/// slot's own partition -- sibling slots of the same loop, other arrayed
/// loops' slots, and scalar loops alike.
///
/// The returned series for loop `id` is flat and step-major: the value at
/// step `s`, slot `k` is `series[s * n_slots + k]` with `n_slots ==
/// loop_partitions[id].len()` (1 for a scalar loop, so the series is just
/// `step_count` long).  Loops whose `loop_score` is absent from `results`
/// (LTM disabled for that loop, or a discovery-mode compilation) are
/// omitted.
///
/// Members are accumulated in `loop_partitions`' insertion order -- loop
/// emission order, slots ascending within a loop -- NOT a re-sort.  The
/// per-partition sum is bit-significant (IEEE-754 addition is
/// non-associative) and emission order is the content-derived order
/// `assign_loop_ids` produces, deterministic across salsa cache
/// invalidations and processes (GH #468); a bare lex sort would order a
/// partition as `b1, b10, b2, ...` and perturb the sum at the ULP.
///
/// # Lone-pin degeneracy
///
/// A modeler-pinned loop (`pin{n}` id) that is the *only* member of its
/// group -- always the case in discovery mode (no enumerated loop scores
/// exist there) and in exhaustive mode whenever the pin is the lone loop
/// through its stock -- has a total equal to its own `|loop_score|`, so its
/// relative score collapses to exactly `+1` (or `-1`, carrying the raw
/// score's sign) whenever the loop is active and `0` (via SAFEDIV-0) when
/// its raw score is `0`.  This is intentional: a fraction-of-all-known-loops
/// normalization is undefined for a group of one.  Callers that want a
/// pinned loop's actual magnitude read its **raw** `loop_score` series
/// ([`compute_raw_loop_score_for_element`]).  Two or more pins (or a pin
/// plus enumerated loops) on stocks in the *same* partition normalize
/// against each other normally.
pub fn compute_rel_loop_scores(
    results: &Results,
    loop_partitions: &IndexMap<String, Vec<Option<usize>>>,
) -> HashMap<String, Vec<f64>> {
    /// One `(loop, slot)` member: its group and its `Results` column.
    struct Member {
        group: NormGroup,
        column: usize,
    }
    // Loops with a `loop_score` column, in emission order, as
    // `(id, n_slots)`; their members are contiguous in `members`, in the
    // same order.
    let mut loops: Vec<(&String, usize)> = Vec::with_capacity(loop_partitions.len());
    let mut members: Vec<Member> = Vec::new();
    for (id, partitions) in loop_partitions {
        let Some(&base) = results.offsets.get(&loop_score_ident(id)) else {
            continue;
        };
        let n_slots = partitions.len().max(1);
        debug_assert!(
            base + n_slots <= results.step_size,
            "loop {id}: {n_slots} slots at column {base} exceed the {} columns per step",
            results.step_size
        );
        loops.push((id, n_slots));
        for k in 0..n_slots {
            let partition = partitions.get(k).copied().flatten();
            members.push(Member {
                group: NormGroup::for_member(partition, members.len()),
                column: base + k,
            });
        }
    }

    let column = |c: usize| results.iter().map(move |row| row[c]);
    let totals = group_totals(
        members.iter().map(|m| (m.group, column(m.column))),
        results.step_count,
    );

    let mut out: HashMap<String, Vec<f64>> = HashMap::with_capacity(loops.len());
    let mut first_member = 0usize;
    for (id, n_slots) in loops {
        let mut series = vec![0.0_f64; results.step_count * n_slots];
        for (k, member) in members[first_member..first_member + n_slots]
            .iter()
            .enumerate()
        {
            let rel = relative_series(column(member.column), &totals[&member.group]);
            for (t, v) in rel.into_iter().enumerate() {
                series[t * n_slots + k] = v;
            }
        }
        first_member += n_slots;
        out.insert(id.clone(), series);
    }
    out
}

/// Collapse a per-slot series laid out as [`compute_rel_loop_scores`]
/// returns it (`series[t * n_slots + k]`, `n_slots = series.len() /
/// step_count`) to one signed value per step: the slot with the largest
/// `|value|`, its sign preserved, the lowest slot index on ties.  A step
/// whose winner is non-finite (every slot `NaN`, or an `Inf`) reports `0.0`,
/// the "inactive" reading the layout and FFI consumers of an importance
/// series want; a `NaN` slot never displaces a finite candidate (the
/// comparison is false).  A scalar loop (`n_slots == 1`) is returned
/// unchanged apart from that non-finite mapping.  An empty series yields an
/// empty result.
pub fn argmax_abs_by_step(series: &[f64], step_count: usize) -> Vec<f64> {
    if series.is_empty() || step_count == 0 {
        return Vec::new();
    }
    let n_slots = (series.len() / step_count).max(1);
    (0..step_count)
        .map(|t| {
            let mut best = 0.0_f64;
            let mut best_abs = -1.0_f64;
            for &v in &series[t * n_slots..(t + 1) * n_slots] {
                // `>` (not `>=`) keeps the lowest-index slot on ties; a NaN
                // compares false and never becomes the winner.
                if v.abs() > best_abs {
                    best_abs = v.abs();
                    best = v;
                }
            }
            if best.is_finite() { best } else { 0.0 }
        })
        .collect()
}

/// [`argmax_abs_by_step`] over every loop of a [`compute_rel_loop_scores`]
/// map: one signed importance series per loop, `step_count` long.  This is
/// what a bare (unsubscripted) arrayed loop id means on every surface -- the
/// layout's `FeedbackLoop::importance_series` and the FFI's
/// `simlin_analyze_get_relative_loop_score("r1")` -- so the two cannot pick
/// different slots.
pub fn aggregate_per_element_argmax_abs(
    per_element_rel_scores: &HashMap<String, Vec<f64>>,
    step_count: usize,
) -> HashMap<String, Vec<f64>> {
    per_element_rel_scores
        .iter()
        .map(|(id, series)| (id.clone(), argmax_abs_by_step(series, step_count)))
        .collect()
}

/// The slot a raw per-element read of a loop with `n_slots` slots resolves
/// `element_index` to.
///
/// - Scalar loops (`n_slots <= 1`) -> `Some(0)`: their single slot answers
///   every element index.
/// - Arrayed loops with `element_index < n_slots` -> `Some(element_index)`.
/// - Arrayed loops with `element_index >= n_slots` -> `None`: the loop has
///   no such element, and the caller must not read past its own columns
///   into a neighboring loop's data.
fn effective_slot(n_slots: usize, element_index: usize) -> Option<usize> {
    if n_slots <= 1 {
        Some(0)
    } else if element_index < n_slots {
        Some(element_index)
    } else {
        None
    }
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

/// Element `element_index` of `loop_id`'s RAW `loop_score` series, with NO
/// normalization (GH #998).  This is the accessor the lone-pin workaround
/// needs -- a modeler-pinned loop alone in its cycle partition has a
/// relative score of exactly `±1` by construction, so its RAW series is the
/// informative one, and a plain `get_series` on the synthetic name resolves
/// to element 0 only.
///
/// Scalar loops (`n_slots <= 1`) read slot 0 whatever the element index; an
/// `element_index` past `n_slots` yields zero-fill rather than reading a
/// neighboring column ([`effective_slot`]); `None` only when the loop's
/// `loop_score` variable is absent from `results`.
pub fn compute_raw_loop_score_for_element(
    results: &Results,
    loop_id: &str,
    n_slots: usize,
    element_index: usize,
) -> Option<Vec<f64>> {
    let off = results.offsets.get(&loop_score_ident(loop_id)).copied()?;
    let Some(slot) = effective_slot(n_slots, element_index) else {
        return Some(vec![0.0; results.step_count]);
    };
    Some(results.iter().map(|row| row[off + slot]).collect())
}

/// The bare-id aggregate over an arrayed loop's RAW `loop_score` slots (GH
/// #998): each step emits the SIGNED value of the slot with the largest
/// magnitude, ties broken to the lowest slot index.  Scalar (`n_slots <= 1`)
/// reduces to identity, so a bare id means the same thing on the raw and
/// relative accessors (the dominant element's contribution, sign preserved).
/// Two deliberate divergences from the relative aggregate
/// ([`argmax_abs_by_step`]): the dominant slot is picked by |raw| here and by
/// |relative| there (the two can select different slots at a step where
/// slots sit in different partitions with different denominators), and a
/// step where EVERY slot is NaN stays NaN here (honest raw data) while the
/// relative aggregate reports 0.0 (its SAFEDIV "inactive" convention).
pub fn compute_raw_loop_score_argmax_abs(
    results: &Results,
    loop_id: &str,
    n_slots: usize,
) -> Option<Vec<f64>> {
    let off = results.offsets.get(&loop_score_ident(loop_id)).copied()?;
    let slots = n_slots.max(1);
    let mut out = Vec::with_capacity(results.step_count);
    for row in results.iter() {
        // A step where EVERY slot is NaN stays NaN: the raw accessor's
        // contract is honest data, and a fabricated finite 0.0 would hide
        // undefined values the per-element accessors report.  A step with
        // any finite slot picks the finite argmax -- NaN slots are skipped
        // by the comparison below.
        let mut best: Option<f64> = None;
        let mut best_abs: f64 = -1.0;
        for k in 0..slots {
            let Some(slot) = effective_slot(n_slots, k) else {
                continue;
            };
            let v = row[off + slot];
            // `>` (not `>=`) keeps the lowest-index slot on ties; a NaN
            // candidate compares false and never becomes the winner.
            if v.abs() > best_abs {
                best_abs = v.abs();
                best = Some(v);
            }
        }
        out.push(best.unwrap_or(f64::NAN));
    }
    Some(out)
}

/// One causal link's input to relative-link-score normalization: the
/// canonical name of the link's `to` target plus its raw LTM link-score
/// series (`None` for an edge with no score series -- a constant/parameter
/// edge, or in exhaustive mode an edge that lies outside every loop).
pub struct RelLinkInput<'a> {
    /// Canonical identifier of the link's `to` (target) variable.  Links
    /// are grouped by this key; the denominator is the per-step sum of
    /// `|score|` over all links sharing it.
    pub to: &'a str,
    /// The link's raw `loop`-link-score series, or `None` when the edge has
    /// no score column.  A `None` link contributes nothing to any
    /// denominator and gets no relative score back.
    pub score: Option<&'a [f64]>,
}

/// Compute the **relative** link score for every input link.
///
/// The raw LTM link score divides by the change in the *target* variable,
/// so links into a near-constant target produce astronomically large raw
/// scores (the denominator approaches zero).  Raw scores are therefore NOT
/// comparable across different targets, and ranking links globally by raw
/// magnitude surfaces numerically degenerate links rather than meaningful
/// ones (GH #652).
///
/// The relative link score fixes this by normalizing, per target and per
/// timestep, against the magnitudes of *all* the target's scored inputs:
///
/// ```text
/// rel[link, t] = score[link, t] / Σ_{j : to(j) == to(link)} |score[j, t]|
/// ```
///
/// This is the link-level analogue of [`compute_rel_loop_scores`]'s
/// per-partition normalization -- the same [`group_totals`] and
/// [`relative_series`], keyed by target instead of by partition -- and
/// yields a value in `[-1, 1]` that *is* comparable across the whole model:
/// the fraction of the target's change attributable to that input.  (LTM
/// ref 13.3 / 13.12, Schoenberg 2020 section 4.)
///
/// **Signed, not absolute.** The literature phrases the relative magnitude
/// as a `[0, 1]` fraction, but we keep the link's sign (so the result is in
/// `[-1, 1]`) for the same reason [`compute_rel_loop_scores`] keeps loop
/// polarity: the sign tells a reader whether the input pushes the target up
/// or down, and dropping it would discard information the raw series
/// carries.  Callers that want the pure magnitude take `.abs()`.
///
/// **NaN/Inf semantics** are the shared accumulator's (GH #542): a `NaN`
/// summand is excluded from the per-target sum (one input being undefined
/// at a step must not poison its siblings' relative scores), while `Inf` is
/// retained (a genuinely diverging input dominates its target, so the
/// finite siblings normalize to `0` and the diverging one to `NaN` via
/// `Inf/Inf`).  A `0.0` denominator yields `0.0` (SAFEDIV-0), and a link's
/// own `NaN` numerator stays `NaN` -- the honest "undefined here" per-link
/// signal.
///
/// **Denominator scope.** The sum runs over the input links that *have* a
/// score series; `None`-score links (constants/parameters; out-of-loop
/// edges in exhaustive mode) contribute nothing.  In **discovery mode**
/// every causal edge is scored, so the denominator is the complete set of
/// the target's inputs and the relative score is exactly "fraction of the
/// target's change attributable to this input".  In **exhaustive mode**
/// only in-loop edges carry score series, so for a target whose inputs are
/// only partially in loops the denominator covers just the scored subset --
/// the relative score is then "fraction among the target's *scored* inputs",
/// a slight overstatement of each link's true share.  This asymmetry is the
/// pragmatic choice (normalize over what is actually scored) rather than
/// inventing zero series for unscored edges; callers ranking links across a
/// large exhaustive-mode model should read it with that caveat.
///
/// The return value is parallel to `links`: `Some(series)` of length
/// `step_count` for each link that had a score, `None` for each link that
/// did not (mirroring the input's `score: None`).  Within a target the
/// denominator accumulates in the caller's link order.
pub fn compute_rel_link_scores(
    links: &[RelLinkInput<'_>],
    step_count: usize,
) -> Vec<Option<Vec<f64>>> {
    let totals = group_totals(
        links
            .iter()
            .filter_map(|link| link.score.map(|s| (link.to, s.iter().copied()))),
        step_count,
    );
    links
        .iter()
        .map(|link| {
            let series = link.score?;
            // Pad a short series with zeros so the output is always
            // `step_count` long (never in production -- every column is
            // `results.step_count` long -- but it keeps the helper total).
            let padded = (0..step_count).map(|t| series.get(t).copied().unwrap_or(0.0));
            Some(relative_series(padded, &totals[link.to]))
        })
        .collect()
}

/// The size of each link's relative-score normalization group (GH #998):
/// for a CONTRIBUTING link, the number of contributing links sharing its
/// `to` target (itself included); `0` for a link that never contributes --
/// no score series, or an all-NaN one.  Parallel to `links`.
///
/// The relative link score is a share WITHIN this group, so a group of ONE
/// reads exactly `±1` at every step by construction -- carrying no ranking
/// information.  Exposing the group size makes that degeneracy detectable:
/// callers ranking links can group by target (the group key is `to`) and
/// treat `group_size == 1` links as trivially self-normalized.  Counting
/// allocated score columns instead of contributors would miss exactly that
/// case: an all-NaN sibling adds no summand to any step's denominator, so
/// the lone finite link still reads `±1` -- and all-NaN series are common
/// on large exhaustive-mode models.
pub fn rel_link_group_sizes(links: &[RelLinkInput<'_>]) -> Vec<usize> {
    // A link CONTRIBUTES when its series adds at least one summand to its
    // target's denominator -- i.e. it has at least one non-NaN entry
    // (`add_to_total` excludes NaN and keeps Inf, and Inf is not NaN). An
    // all-NaN series never contributes, so counting it would over-report
    // competition: its finite sibling normalizes against itself alone and
    // reads +/-1 by construction, which is exactly the degeneracy this
    // count exists to flag. A never-contributing link (no series, or
    // all-NaN) reports 0, so `count > 0` means "carries a usable score" and
    // `count == 1` means "with no competition". Residual, documented rather
    // than modeled: NaN exclusion is per STEP, so a link that is NaN at
    // some steps and finite at others counts here yet leaves its siblings
    // momentarily unopposed at its NaN steps -- a scalar cannot carry that.
    let contributes =
        |link: &RelLinkInput<'_>| link.score.is_some_and(|s| s.iter().any(|v| !v.is_nan()));
    let mut contributing_by_target: HashMap<&str, usize> = HashMap::new();
    for link in links {
        if contributes(link) {
            *contributing_by_target.entry(link.to).or_insert(0) += 1;
        }
    }
    links
        .iter()
        .map(|link| {
            if contributes(link) {
                contributing_by_target.get(link.to).copied().unwrap_or(0)
            } else {
                0
            }
        })
        .collect()
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
    fn mapping(pairs: &[(&str, Option<usize>)]) -> IndexMap<String, Vec<Option<usize>>> {
        pairs
            .iter()
            .map(|(id, p)| ((*id).to_string(), vec![*p]))
            .collect()
    }

    /// Build a per-slot `loop_partitions` mapping from `(loop_id, slots)`
    /// pairs -- for tests that need genuinely multi-slot A2A loops.
    fn mapping_per_slot(
        pairs: &[(&str, Vec<Option<usize>>)],
    ) -> IndexMap<String, Vec<Option<usize>>> {
        pairs
            .iter()
            .map(|(id, slots)| ((*id).to_string(), slots.clone()))
            .collect()
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

    /// Naive, from-first-principles reference for
    /// [`compute_rel_loop_scores`].  Where the engine accumulates group
    /// totals in one pass, this loops directly over each `(loop, slot,
    /// step)` and re-derives the SAFEDIV denominator by scanning *every*
    /// other `(loop, slot)` and asking "is it in my partition?" -- a
    /// structurally different computation, so the proptest is a real
    /// oracle, not a paraphrase of the implementation.
    ///
    /// Member rule, spelled out inline rather than via the engine helpers:
    /// `(j, k')` is in `(i, k)`'s group iff both resolve to the same
    /// `Some(p)`; an unresolved (`None`) slot is in a group of its own
    /// (GH #750), so it normalizes against itself only.  `NaN` summands are
    /// excluded (GH #542) and `Inf` kept, re-derived inline too.
    ///
    /// `slots[i]` is loop `i`'s per-slot partition vector (length 1 for a
    /// scalar loop); `series[i][step][slot]` its `loop_score`.  Returns one
    /// flat `Vec<f64>` per loop in `loop_ids` order, `step * n_slots_i + k`
    /// indexed.  The denominator walks members in ascending `(loop, slot)`
    /// order -- the production accumulation order -- so the comparison is
    /// bit-exact, not merely close.
    fn reference_rel_loop_scores(
        loop_ids: &[String],
        slots: &[Vec<Option<usize>>],
        series: &[Vec<Vec<f64>>],
        step_count: usize,
    ) -> Vec<Vec<f64>> {
        let n = loop_ids.len();
        let n_slots: Vec<usize> = slots.iter().map(|v| v.len().max(1)).collect();
        let partition =
            |i: usize, k: usize| -> Option<usize> { slots[i].get(k).copied().flatten() };
        let mut out: Vec<Vec<f64>> = (0..n)
            .map(|i| vec![0.0_f64; step_count * n_slots[i]])
            .collect();
        for i in 0..n {
            for k in 0..n_slots[i] {
                // The members of (i, k)'s group, in ascending (loop, slot)
                // order: every slot sharing its resolved partition, or just
                // itself when unresolved.
                let members: Vec<(usize, usize)> = match partition(i, k) {
                    Some(p) => (0..n)
                        .flat_map(|j| (0..n_slots[j]).map(move |kk| (j, kk)))
                        .filter(|&(j, kk)| partition(j, kk) == Some(p))
                        .collect(),
                    None => vec![(i, k)],
                };
                for step in 0..step_count {
                    let denom: f64 = members
                        .iter()
                        .map(|&(j, kk)| {
                            let v = series[j][step][kk];
                            if v.is_nan() { 0.0 } else { v.abs() }
                        })
                        .sum();
                    let num = series[i][step][k];
                    out[i][step * n_slots[i] + k] = if denom == 0.0 { 0.0 } else { num / denom };
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

    /// GH #468 consumer-side guard: `compute_rel_loop_scores` must accumulate
    /// its per-partition `Σ|loop_score|` denominator in the `loop_partitions`
    /// IndexMap's *emission* (insertion) order, NOT a re-sort of the loop ids.
    /// IEEE-754 addition is non-associative, so a lex re-sort would perturb the
    /// denominator at the ULP and break the bit-for-bit stability of the sum.
    ///
    /// The fixture puts 12 same-prefix loops (`r1..r12`) in one partition with
    /// scores chosen so the emission-order abs-sum and the lex-order abs-sum are
    /// bit-DIFFERENT: eleven `1.0`s plus one `1e16` (at `r12`). In emission order
    /// the eleven `1.0`s accumulate to `11.0` BEFORE the `1e16` swamps them, so
    /// `1e16 + 11.0` keeps more of them; in lex order (`r1, r10, r11, r12, r2,
    /// ...`) the `1e16` lands 4th and absorbs the trailing `1.0`s. The test
    /// asserts the production output matches the emission-order denominator
    /// bit-for-bit, and that the emission-order denominator differs bit-for-bit
    /// from the lex-order one (so the equality assertion has teeth -- a
    /// reintroduced lex sort would fail the first assertion).
    #[test]
    fn rel_loop_scores_accumulate_in_emission_order_not_lex() {
        // r1..r11 score 1.0; r12 scores 1e16. All in partition 0. The fixture
        // (and `mapping` below) inserts them in emission order r1..r12.
        let ids: Vec<String> = (1..=12).map(|i| format!("r{i}")).collect();
        let scores: Vec<f64> = (1..=12).map(|i| if i == 12 { 1e16 } else { 1.0 }).collect();

        // Single-timestep series, one value per loop.
        let series_storage: Vec<[f64; 1]> = scores.iter().map(|&s| [s]).collect();
        let result_pairs: Vec<(&str, &[f64])> = ids
            .iter()
            .zip(series_storage.iter())
            .map(|(id, ser)| (id.as_str(), &ser[..]))
            .collect();
        let results = make_results_for_loops(&result_pairs);

        // Insert into `loop_partitions` in emission order r1..r12, all part 0.
        let partition_pairs: Vec<(&str, Option<usize>)> =
            ids.iter().map(|id| (id.as_str(), Some(0))).collect();
        let partitions = mapping(&partition_pairs);

        let scored = compute_rel_loop_scores(&results, &partitions);

        // Reference denominator accumulated in EMISSION order (r1..r12): this is
        // exactly the order `compute_rel_loop_scores` must walk.
        let mut emission_denom = 0.0_f64;
        for &s in &scores {
            emission_denom += s.abs();
        }

        // Reference denominator accumulated in LEX order of the ids (r1, r10,
        // r11, r12, r2, ..., r9): what a re-sort would (wrongly) produce.
        let mut lex_idx: Vec<usize> = (0..ids.len()).collect();
        lex_idx.sort_by(|&a, &b| ids[a].cmp(&ids[b]));
        let mut lex_denom = 0.0_f64;
        for &i in &lex_idx {
            lex_denom += scores[i].abs();
        }

        // Teeth: the two orders must genuinely differ at the bit level, else
        // the equality assertion below could pass under either accumulation
        // order and the test would not be guarding emission order at all.
        assert_ne!(
            emission_denom.to_bits(),
            lex_denom.to_bits(),
            "fixture must make emission-order and lex-order abs-sums bit-different; \
             without that this test has no teeth (emission={emission_denom:?}, \
             lex={lex_denom:?})"
        );

        // Each loop's relative score must equal its raw score over the
        // EMISSION-order denominator, bit-for-bit. A reintroduced lex sort would
        // accumulate `lex_denom` instead and fail here.
        for (id, &score) in ids.iter().zip(scores.iter()) {
            let rel = scored
                .get(id)
                .unwrap_or_else(|| panic!("loop {id} missing"));
            let expected = score / emission_denom;
            assert_eq!(
                rel[0].to_bits(),
                expected.to_bits(),
                "loop {id}: rel score must use the emission-order denominator \
                 bit-for-bit (got {:?}, expected {expected:?}; lex denom would be {lex_denom:?})",
                rel[0]
            );
        }
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
        // (a NaN score marks its edge inactive at that step).
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
    fn nan_slot_isolated_to_its_own_member() {
        // Arrayed twin of `nan_loop_score_isolated_to_its_own_loop`: a NaN
        // at one (loop, slot) is dropped from the partition's total, so
        // every other member of the partition -- the loop's own sibling
        // slot included -- normalizes against the healthy total.  Two
        // coupled A2A loops, 2 slots each, all four slots in partition 0;
        // plant a NaN at A's slot 0 at step 0.
        let n_slots: usize = 2;
        // A = [NaN, 4] then [9, 4]; B = [3, 6] then [3, 6].
        let loop_data = vec![
            vec![vec![f64::NAN, 4.0], vec![9.0, 4.0]],
            vec![vec![3.0, 6.0], vec![3.0, 6.0]],
        ];
        let results = make_arrayed_results(&["A", "B"], &[n_slots, n_slots], &loop_data);
        let partitions =
            mapping_per_slot(&[("A", vec![Some(0); n_slots]), ("B", vec![Some(0); n_slots])]);

        let rel = compute_rel_loop_scores(&results, &partitions);
        let a = rel.get("A").unwrap();
        let b = rel.get("B").unwrap();
        let at = |step: usize, k: usize| step * n_slots + k;

        // step 0: total over the partition = |4| + |3| + |6| = 13 (the NaN
        // slot dropped).  A[0] = NaN/13 = NaN; the other three members
        // share the healthy 13.
        assert!(a[at(0, 0)].is_nan(), "the NaN slot keeps its own NaN");
        assert!(
            (a[at(0, 1)] - 4.0 / 13.0).abs() < 1e-12,
            "got {}",
            a[at(0, 1)]
        );
        assert!(
            (b[at(0, 0)] - 3.0 / 13.0).abs() < 1e-12,
            "got {}",
            b[at(0, 0)]
        );
        assert!(
            (b[at(0, 1)] - 6.0 / 13.0).abs() < 1e-12,
            "got {}",
            b[at(0, 1)]
        );
        // step 1: all finite; total = 9 + 4 + 3 + 6 = 22.
        assert!((a[at(1, 0)] - 9.0 / 22.0).abs() < 1e-12);
        assert!((a[at(1, 1)] - 4.0 / 22.0).abs() < 1e-12);
        assert!((b[at(1, 0)] - 3.0 / 22.0).abs() < 1e-12);
        assert!((b[at(1, 1)] - 6.0 / 22.0).abs() < 1e-12);
    }

    #[test]
    fn inf_loop_score_kept_in_denominator() {
        // An +Inf loop_score is REAL signal: at a dominance inflection a
        // raw loop score legitimately diverges (the link-score
        // denominators go to zero there).  Unlike a NaN, an Inf is NOT
        // filtered from the partition sum -- it stays, so the dominated
        // siblings correctly go to 0 (finite/Inf) and the dominant loop
        // momentarily reads NaN (Inf/Inf).
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

    /// A finite partition total that overflows saturates to `f64::MAX`
    /// rather than becoming `Inf`: every summand was finite, so the
    /// members keep finite (tiny) shares instead of all reading `0`.  A
    /// genuine `Inf` summand still makes the total `Inf`.
    #[test]
    fn group_totals_saturate_on_finite_overflow_but_keep_inf() {
        let totals = group_totals(
            [
                (0usize, vec![f64::MAX, 1.0]),
                (0usize, vec![f64::MAX, f64::INFINITY]),
            ],
            2,
        );
        let total = &totals[&0];
        assert_eq!(total[0], f64::MAX, "finite overflow saturates");
        assert_eq!(total[1], f64::INFINITY, "a real Inf summand is kept");
    }

    #[test]
    fn unpartitioned_loops_normalize_independently() {
        // GH #750: a `None` partition means "no provable stock-to-stock
        // coupling" (module-internal-stock loops, PREVIOUS-lagged stockless
        // loops), so two `None`-partition loops must NOT cross-normalize --
        // they may be entirely unrelated subsystems.  Each gets its own
        // singleton group, collapsing to the documented lone-pin degeneracy
        // (sign-preserving +/-1, or 0 via SAFEDIV-0).  A shared default
        // bucket would pool them (rel = 0.75 / 0.25 here), the GH #487-class
        // cross-pollution.
        let series_a = &[3.0, 0.0][..];
        let series_b = &[-1.0, 2.0][..];
        let results = make_results_for_loops(&[("A", series_a), ("B", series_b)]);
        let partitions = mapping(&[("A", None), ("B", None)]);

        let scored = compute_rel_loop_scores(&results, &partitions);
        let rel_a = scored.get("A").unwrap();
        let rel_b = scored.get("B").unwrap();
        // Each loop's denom is its own |score|: sign in, SAFEDIV-0 on zero.
        assert!((rel_a[0] - 1.0).abs() < 1e-12, "got {}", rel_a[0]);
        assert_eq!(rel_a[1], 0.0, "a zero score normalizes to 0 via SAFEDIV-0");
        assert!((rel_b[0] - (-1.0)).abs() < 1e-12, "got {}", rel_b[0]);
        assert!((rel_b[1] - 1.0).abs() < 1e-12, "got {}", rel_b[1]);
    }

    #[test]
    fn unpartitioned_loops_resolved_partitions_unaffected() {
        // The singleton rule applies ONLY to `None`-partition loops: loops
        // with a resolved partition keep normalizing against their partition
        // siblings, bit-for-bit (the GH #468 emission-order sum).
        let series_a = &[3.0][..];
        let series_b = &[1.0][..];
        let series_c = &[2.0][..];
        let results = make_results_for_loops(&[("A", series_a), ("B", series_b), ("C", series_c)]);
        let partitions = mapping(&[("A", Some(0)), ("B", Some(0)), ("C", None)]);

        let scored = compute_rel_loop_scores(&results, &partitions);
        assert!((scored.get("A").unwrap()[0] - 0.75).abs() < 1e-12);
        assert!((scored.get("B").unwrap()[0] - 0.25).abs() < 1e-12);
        assert!((scored.get("C").unwrap()[0] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn unpartitioned_arrayed_slots_are_each_solo() {
        // Arrayed twin of `unpartitioned_loops_normalize_independently`: an
        // unresolved slot is a Solo member on its own, so the two `None`
        // slots of one arrayed loop do not normalize against each other,
        // and a `None` scalar loop does not join either of them.
        let n_slots: usize = 2;
        // One step: A is scalar (score 3); B is arrayed over 2 slots (4, 6).
        let loop_data = vec![vec![vec![3.0]], vec![vec![4.0, 6.0]]];
        let results = make_arrayed_results(&["A", "B"], &[1, n_slots], &loop_data);
        let partitions = mapping_per_slot(&[("A", vec![None]), ("B", vec![None, None])]);

        let rel = compute_rel_loop_scores(&results, &partitions);
        let a = rel.get("A").unwrap();
        let b = rel.get("B").unwrap();
        assert_eq!(a.len(), 1, "a scalar loop has exactly one series");
        assert!((a[0] - 1.0).abs() < 1e-12, "got {}", a[0]);
        assert_eq!(b.len(), n_slots);
        assert!((b[0] - 1.0).abs() < 1e-12, "got {}", b[0]);
        assert!((b[1] - 1.0).abs() < 1e-12, "got {}", b[1]);
    }

    /// The headline rule: every `(loop, slot)` of a partition shares ONE
    /// denominator.  A coupled A2A loop (both slots in partition 0) and a
    /// scalar loop in the same partition: at each step the total is
    /// `|A[0]| + |A[1]| + |S|`, the scalar loop has exactly one series, and
    /// the three shares sum to 1.  Grouping by `(partition, slot)` instead
    /// would give A[0] a denominator of `|A[0]| + |S|` and A[1] one of
    /// `|A[1]| + |S|`, each missing a member, and would hand the scalar loop
    /// two different series.
    #[test]
    fn arrayed_slots_and_scalar_loops_share_one_partition_denominator() {
        let n_slots: usize = 2;
        // step 0: A = [3, 6], S = 1  -> total 10
        // step 1: A = [1, 4], S = -5 -> total 10
        let loop_data = vec![
            vec![vec![3.0, 6.0], vec![1.0, 4.0]],
            vec![vec![1.0], vec![-5.0]],
        ];
        let results = make_arrayed_results(&["A", "S"], &[n_slots, 1], &loop_data);
        let partitions = mapping_per_slot(&[("A", vec![Some(0); n_slots]), ("S", vec![Some(0)])]);

        let rel = compute_rel_loop_scores(&results, &partitions);
        let a = rel.get("A").unwrap();
        let s = rel.get("S").unwrap();
        assert_eq!(a.len(), 2 * n_slots);
        assert_eq!(s.len(), 2, "a scalar loop has one series, step_count long");

        let at = |step: usize, k: usize| step * n_slots + k;
        assert!((a[at(0, 0)] - 0.3).abs() < 1e-12, "got {}", a[at(0, 0)]);
        assert!((a[at(0, 1)] - 0.6).abs() < 1e-12, "got {}", a[at(0, 1)]);
        assert!((s[0] - 0.1).abs() < 1e-12, "got {}", s[0]);
        assert!((a[at(1, 0)] - 0.1).abs() < 1e-12, "got {}", a[at(1, 0)]);
        assert!((a[at(1, 1)] - 0.4).abs() < 1e-12, "got {}", a[at(1, 1)]);
        assert!((s[1] - (-0.5)).abs() < 1e-12, "got {}", s[1]);
    }

    /// Two arrayed loops of different widths in one partition: all five
    /// slots share the denominator.  A slot index means nothing across
    /// loops here -- A's slot 2 has no counterpart in B and still competes
    /// with every member.
    #[test]
    fn arrayed_loops_of_different_widths_share_a_partition() {
        // One step: A = [1, 2, 3] (3 slots), B = [4, 10] (2 slots); total 20.
        let loop_data = vec![vec![vec![1.0, 2.0, 3.0]], vec![vec![4.0, 10.0]]];
        let results = make_arrayed_results(&["A", "B"], &[3, 2], &loop_data);
        let partitions = mapping_per_slot(&[("A", vec![Some(0); 3]), ("B", vec![Some(0); 2])]);

        let rel = compute_rel_loop_scores(&results, &partitions);
        let a = rel.get("A").unwrap();
        let b = rel.get("B").unwrap();
        assert_eq!(a.len(), 3);
        assert_eq!(b.len(), 2);
        for (got, expected) in a.iter().zip([0.05, 0.10, 0.15]) {
            assert!(
                (got - expected).abs() < 1e-12,
                "got {got}, expected {expected}"
            );
        }
        for (got, expected) in b.iter().zip([0.20, 0.50]) {
            assert!(
                (got - expected).abs() < 1e-12,
                "got {got}, expected {expected}"
            );
        }
    }

    /// The GH #487 case: two A2A loops over element-wise-*uncoupled*
    /// dimensions -- each slot of each loop is in its own partition, and no
    /// slot of A shares a partition with any slot of B.  Each slot
    /// therefore normalizes against itself only, so every rel score is
    /// ±1.0 -- the two loops do NOT cross-normalize.
    #[test]
    fn uncoupled_a2a_slots_self_normalize() {
        let step_count: usize = 3;
        // A has 2 slots (partitions 0, 1), B has 3 slots (partitions 2, 3,
        // 4); distinct, non-zero, per-step-varying magnitudes.
        let a_data: Vec<Vec<f64>> = (0..step_count)
            .map(|step| (0..2).map(|k| ((step + 2) * (k + 1)) as f64).collect())
            .collect();
        let b_data: Vec<Vec<f64>> = (0..step_count)
            .map(|step| (0..3).map(|k| -(((step + 3) * (k + 1)) as f64)).collect())
            .collect();
        let results = make_arrayed_results(&["A", "B"], &[2, 3], &[a_data, b_data]);
        let partitions = mapping_per_slot(&[
            ("A", vec![Some(0), Some(1)]),
            ("B", vec![Some(2), Some(3), Some(4)]),
        ]);

        let rel = compute_rel_loop_scores(&results, &partitions);
        let a = rel.get("A").expect("A must have a series");
        let b = rel.get("B").expect("B must have a series");
        assert_eq!(a.len(), step_count * 2);
        assert_eq!(b.len(), step_count * 3);
        for &v in a.iter().chain(b.iter()) {
            assert!(
                (v.abs() - 1.0).abs() < 1e-12,
                "uncoupled A2A slot should self-normalize to ±1.0, got {v}"
            );
        }
    }

    /// The discovery path's inputs -- `(time, score)` pairs per loop with a
    /// per-loop group -- normalize through the same two functions the
    /// exhaustive owner uses, so feeding the same numbers to both yields the
    /// same relative series.  This is the parity the two surfaces rely on.
    #[test]
    fn discovery_style_series_normalize_through_the_same_functions() {
        let scores_a: Vec<(f64, f64)> = vec![(0.0, 1.0), (1.0, -4.0), (2.0, 0.0)];
        let scores_b: Vec<(f64, f64)> = vec![(0.0, 3.0), (1.0, 4.0), (2.0, 0.0)];
        let groups = [
            NormGroup::for_member(Some(0), 0),
            NormGroup::for_member(Some(0), 1),
        ];
        fn score(pair: &(f64, f64)) -> f64 {
            pair.1
        }
        let totals = group_totals(
            [
                (groups[0], scores_a.iter().map(score)),
                (groups[1], scores_b.iter().map(score)),
            ],
            3,
        );
        let rel_a = relative_series(scores_a.iter().map(score), &totals[&groups[0]]);
        let rel_b = relative_series(scores_b.iter().map(score), &totals[&groups[1]]);

        let results = make_results_for_loops(&[("A", &[1.0, -4.0, 0.0]), ("B", &[3.0, 4.0, 0.0])]);
        let owner = compute_rel_loop_scores(&results, &mapping(&[("A", Some(0)), ("B", Some(0))]));
        assert_eq!(&rel_a, owner.get("A").unwrap());
        assert_eq!(&rel_b, owner.get("B").unwrap());
        assert_eq!(rel_a, vec![0.25, -0.5, 0.0]);
        assert_eq!(rel_b, vec![0.75, 0.5, 0.0]);
    }

    /// `argmax_abs_by_step` on a 3-slot series: the slot with the largest
    /// magnitude wins each step, sign preserved.
    #[test]
    fn aggregate_pure_arrayed_argmax_abs() {
        let mut per_elem = HashMap::new();
        // 2 steps × 3 elements: layout step*3 + k.
        //   step 0: [0.1,  0.5, -0.2]  -> argmax-abs picks 0.5
        //   step 1: [0.3, -0.4, 0.0]   -> argmax-abs picks -0.4
        per_elem.insert("L".to_string(), vec![0.1, 0.5, -0.2, 0.3, -0.4, 0.0]);

        let out = aggregate_per_element_argmax_abs(&per_elem, 2);
        let agg = out.get("L").expect("L must have aggregate");
        assert_eq!(agg, &vec![0.5, -0.4]);
    }

    /// A scalar loop's series (stride 1) passes through unchanged.
    #[test]
    fn aggregate_scalar_series_is_identity() {
        let mut per_elem = HashMap::new();
        per_elem.insert("A".to_string(), vec![0.10, 0.15, -0.12, 0.18]);
        let out = aggregate_per_element_argmax_abs(&per_elem, 4);
        assert_eq!(out.get("A").unwrap(), &vec![0.10, 0.15, -0.12, 0.18]);
    }

    /// Ties (two elements with equal |rel|) are broken deterministically:
    /// the lowest slot index wins.
    #[test]
    fn argmax_abs_ties_broken_by_lowest_index() {
        assert_eq!(argmax_abs_by_step(&[0.4, -0.4], 1), vec![0.4]);
    }

    /// Non-finite values (NaN, ±Inf) map to 0.0 in the output; a NaN never
    /// displaces a finite candidate.
    #[test]
    fn aggregate_filters_non_finite() {
        let mut per_elem = HashMap::new();
        per_elem.insert(
            "L".to_string(),
            vec![f64::NAN, 0.5, f64::INFINITY, -f64::INFINITY],
        );

        let out = aggregate_per_element_argmax_abs(&per_elem, 2);
        let agg = out.get("L").expect("L must have aggregate");
        // step 0: [NaN, 0.5] -> 0.5 (NaN compares false).
        // step 1: [Inf, -Inf] -> both non-finite -> 0.0.
        assert_eq!(agg, &vec![0.5, 0.0]);
    }

    /// An empty per-loop series yields an empty aggregate (the layout's
    /// "no series" branch).
    #[test]
    fn aggregate_empty_series_yields_empty_output() {
        let mut per_elem = HashMap::new();
        per_elem.insert("L".to_string(), Vec::new());
        let out = aggregate_per_element_argmax_abs(&per_elem, 5);
        assert!(out.get("L").unwrap().is_empty());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        /// `compute_rel_loop_scores` must match the naive per-member
        /// reference for arbitrary per-slot partition vectors -- coupled
        /// (all entries the same `Some(p)`), uncoupled (distinct `Some(p)`
        /// per slot), `None`-laced, and scalar -- mixed across loops of
        /// different widths so slots of different loops really do share
        /// partitions.  The one-pass accumulation and the scan-every-member
        /// reference are computed independently, so any divergence in the
        /// grouping, the Solo rule, or the SAFEDIV-0 handling shows up here.
        ///
        /// Per-loop `spec[i] = (kind, len, base, vals)` builds the
        /// partition vector:
        ///   - kind 0: scalar `[Some(base)]`.
        ///   - kind 1: scalar `[None]`.
        ///   - kind 2: coupled arrayed `[Some(base); len]`.
        ///   - kind 3: uncoupled arrayed `[Some(base), Some(base+1), ...]`
        ///     (distinct consecutive partitions).
        ///   - kind 4: `None`-laced arrayed -- `vals[k]` chooses
        ///     `Some(vals[k])` or `None` per slot: the vector
        ///     `partition_for_loop` returns for an A2A loop some of whose
        ///     slots resolve to no parent-level stock partition (a slot
        ///     whose only state is module-internal), the rest to
        ///     arbitrary partitions.
        /// Partition indices stay in a small pool (so coupling across
        /// *different* loops actually happens); lengths stay tiny so 128
        /// cases run in well under a second on a debug build.
        #[test]
        fn rel_loop_scores_match_naive_reference(
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
            let loop_partitions: IndexMap<String, Vec<Option<usize>>> = loop_ids
                .iter()
                .zip(slots.iter())
                .map(|(id, v)| (id.clone(), v.clone()))
                .collect();

            let actual = compute_rel_loop_scores(&results, &loop_partitions);
            let expected = reference_rel_loop_scores(&loop_ids, &slots, &series, num_steps);

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
                    // Both walk the members in ascending (loop, slot) order,
                    // so the result is bit-identical, not merely close.
                    prop_assert_eq!(
                        av, ev,
                        "loop {} flat-index {}: actual {} vs reference {}", id, idx, av, ev
                    );
                }
            }

            // The partition identity: at every step where a group's total
            // is finite and non-zero, the magnitudes of its members' shares
            // sum to exactly 1.  Groups are re-derived here from the
            // partition vectors (a Solo member is its own group).
            let mut group_of: Vec<(NormGroup, usize, usize)> = Vec::new();
            for (i, v) in slots.iter().enumerate() {
                for k in 0..n_slots[i] {
                    let g = NormGroup::for_member(v.get(k).copied().flatten(), group_of.len());
                    group_of.push((g, i, k));
                }
            }
            let mut groups: HashMap<NormGroup, Vec<(usize, usize)>> = HashMap::new();
            for &(g, i, k) in &group_of {
                groups.entry(g).or_default().push((i, k));
            }
            for members in groups.values() {
                for step in 0..num_steps {
                    let total: f64 = members
                        .iter()
                        .map(|&(i, k)| series[i][step][k].abs())
                        .sum();
                    if total == 0.0 {
                        continue;
                    }
                    let share: f64 = members
                        .iter()
                        .map(|&(i, k)| actual[&loop_ids[i]][step * n_slots[i] + k].abs())
                        .sum();
                    prop_assert!(
                        (share - 1.0).abs() < 1e-9,
                        "step {}: partition shares sum to {} not 1", step, share
                    );
                }
            }
        }
    }

    /// `LoopElementIndex` for a scalar loop reports empty dimensions
    /// and `n_slots = 1`.  Used by the libsimlin FFI dispatch to detect
    /// "this loop is not arrayed, reject subscripted IDs."
    #[test]
    fn loop_element_index_scalar_loop() {
        let ltm_vars = vec![crate::db::LtmSyntheticVar {
            name: "$\u{205A}ltm\u{205A}loop_score\u{205A}r1".to_string(),
            equation: crate::db::LtmEquation::scalar("1.0".to_string()),
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
            equation: crate::db::LtmEquation::scalar("1.0".to_string()),
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
            equation: crate::db::LtmEquation::scalar("1.0".to_string()),
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
            equation: crate::db::LtmEquation::scalar("1.0".to_string()),
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
            equation: crate::db::LtmEquation::scalar("1.0".to_string()),
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
            equation: crate::db::LtmEquation::scalar("1.0".to_string()),
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
            equation: crate::db::LtmEquation::scalar("1.0".to_string()),
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
            equation: crate::db::LtmEquation::scalar("1.0".to_string()),
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
            equation: crate::db::LtmEquation::scalar("1.0".to_string()),
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
                equation: crate::db::LtmEquation::scalar("1.0".to_string()),
                dimensions: vec![],
                compile_directly: false,
            },
            crate::db::LtmSyntheticVar {
                name: "$\u{205A}ltm\u{205A}loop_score\u{205A}r1".to_string(),
                equation: crate::db::LtmEquation::scalar("1.0".to_string()),
                dimensions: vec![],
                compile_directly: false,
            },
            crate::db::LtmSyntheticVar {
                name: "$\u{205A}ltm\u{205A}path\u{205A}foo\u{205A}0".to_string(),
                equation: crate::db::LtmEquation::scalar("1.0".to_string()),
                dimensions: vec![],
                compile_directly: false,
            },
        ];
        let project_dims: Vec<crate::datamodel::Dimension> = vec![];
        let index = build_loop_element_index(&ltm_vars, &project_dims);
        assert_eq!(index.len(), 1);
        assert!(index.contains_key("r1"));
    }

    // --- Raw per-element loop scores (GH #998) ---------------------------

    #[test]
    fn raw_loop_score_for_element_reads_the_exact_slot() {
        // A 2-slot arrayed loop: element k must read column off+k verbatim,
        // with no normalization (this is the RAW series the lone-pin
        // workaround needs; the relative accessor divides by the partition
        // denominator).
        let slot0 = [10.0, -20.0, 30.0];
        let slot1 = [1.0, 2.0, -3.0];
        let results = make_results_for_arrayed_loop("r1", &[&slot0, &slot1]);
        let s0 = compute_raw_loop_score_for_element(&results, "r1", 2, 0).unwrap();
        let s1 = compute_raw_loop_score_for_element(&results, "r1", 2, 1).unwrap();
        assert_eq!(s0, slot0.to_vec());
        assert_eq!(s1, slot1.to_vec());
    }

    #[test]
    fn raw_loop_score_argmax_abs_picks_dominant_slot_signed() {
        // Bare-id aggregate on an arrayed loop: each step emits the SIGNED
        // value of the slot with the largest |raw| (mirroring the relative
        // accessor's bare-id semantics), ties to the lowest slot.
        let slot0 = [10.0, -1.0, 5.0];
        let slot1 = [-2.0, 7.0, -5.0];
        let results = make_results_for_arrayed_loop("r1", &[&slot0, &slot1]);
        let agg = compute_raw_loop_score_argmax_abs(&results, "r1", 2).unwrap();
        // step 0: |10| > |-2| -> 10; step 1: |7| > |-1| -> 7;
        // step 2: tie |5| == |-5| -> lowest slot wins -> 5.
        assert_eq!(agg, vec![10.0, 7.0, 5.0]);
    }

    #[test]
    fn raw_loop_score_scalar_is_identity_on_both_paths() {
        let series = [4.0, -5.0];
        let results = make_results_for_loops(&[("b1", &series)]);
        assert_eq!(
            compute_raw_loop_score_for_element(&results, "b1", 1, 0).unwrap(),
            series.to_vec()
        );
        assert_eq!(
            compute_raw_loop_score_argmax_abs(&results, "b1", 1).unwrap(),
            series.to_vec()
        );
    }

    #[test]
    fn raw_loop_score_absent_loop_is_none() {
        let results = make_results_for_loops(&[("r1", &[1.0])]);
        assert!(compute_raw_loop_score_for_element(&results, "nope", 1, 0).is_none());
        assert!(compute_raw_loop_score_argmax_abs(&results, "nope", 1).is_none());
    }

    #[test]
    fn raw_loop_score_out_of_range_element_is_zero_fill() {
        // `effective_slot`'s convention: an
        // ARRAYED loop queried past its own slot count yields zeros, not a
        // read of a neighboring loop's column -- while a SCALAR loop
        // broadcasts any element index to its single slot.
        let slot0 = [1.0, 2.0];
        let slot1 = [3.0, 4.0];
        let results = make_results_for_arrayed_loop("r1", &[&slot0, &slot1]);
        assert_eq!(
            compute_raw_loop_score_for_element(&results, "r1", 2, 5).unwrap(),
            vec![0.0, 0.0]
        );
        let scalar = make_results_for_loops(&[("b1", &[7.0, 8.0])]);
        assert_eq!(
            compute_raw_loop_score_for_element(&scalar, "b1", 1, 3).unwrap(),
            vec![7.0, 8.0],
            "a scalar loop broadcasts every element index to slot 0"
        );
    }

    /// Like `make_results_for_loops`, but one loop with N contiguous slots
    /// (the arrayed loop_score layout: columns off..off+n_slots).
    fn make_results_for_arrayed_loop(id: &str, slots: &[&[f64]]) -> Results {
        assert!(!slots.is_empty());
        let step_count = slots[0].len();
        let step_size = slots.len() + 1;
        let mut data = vec![0.0_f64; step_count * step_size];
        let mut offsets: HashMap<Ident<Canonical>, usize> = HashMap::new();
        offsets.insert(Ident::new("time"), 0);
        offsets.insert(loop_score_ident(id), 1);
        for (step, row) in data.chunks_mut(step_size).enumerate() {
            row[0] = step as f64;
            for (k, slot) in slots.iter().enumerate() {
                row[1 + k] = slot[step];
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

    #[test]
    fn raw_loop_score_argmax_abs_preserves_all_nan_steps() {
        // A step where EVERY slot's raw score is NaN must stay NaN in the
        // bare-id aggregate: the raw accessor's contract is honest data, and
        // a fabricated 0.0 would hide undefined values the per-element
        // accessors report (PR #1003 codex review). A step with at least
        // one finite slot still picks the finite argmax (NaN slots skipped).
        let slot0 = [f64::NAN, f64::NAN, 3.0];
        let slot1 = [f64::NAN, -2.0, f64::NAN];
        let results = make_results_for_arrayed_loop("r1", &[&slot0, &slot1]);
        let agg = compute_raw_loop_score_argmax_abs(&results, "r1", 2).unwrap();
        assert!(
            agg[0].is_nan(),
            "all-NaN step must stay NaN, got {}",
            agg[0]
        );
        assert_eq!(agg[1], -2.0, "the finite slot wins over a NaN sibling");
        assert_eq!(agg[2], 3.0);
    }

    // --- Link normalization-group sizes (GH #998) ------------------------

    #[test]
    fn rel_link_group_sizes_count_scored_siblings_per_target() {
        let s = [1.0];
        let links = [
            RelLinkInput {
                to: "z",
                score: Some(&s),
            },
            RelLinkInput {
                to: "z",
                score: Some(&s),
            },
            // Unscored sibling: contributes to no group, gets 0 back.
            RelLinkInput {
                to: "z",
                score: None,
            },
            // A lone scored input of its own target: group of ONE -- the
            // ±1-by-construction case callers need to be able to detect.
            RelLinkInput {
                to: "y",
                score: Some(&s),
            },
        ];
        assert_eq!(rel_link_group_sizes(&links), vec![2, 2, 0, 1]);
    }

    #[test]
    fn rel_link_group_sizes_ignore_never_contributing_siblings() {
        // An ALL-NaN score series never adds a summand to the target's
        // denominator, so its finite sibling is normalized against itself
        // alone and reads +/-1 by construction -- the count must say 1, not
        // 2, or the field misses exactly the degeneracy it exists to flag
        // (PR #1003 codex review). The all-NaN link itself reports 0, like
        // an unscored link: count > 0 means "this link carries a usable
        // score", and count == 1 means "with no competition".
        let finite = [4.0, 5.0];
        let all_nan = [f64::NAN, f64::NAN];
        // A PARTIALLY-NaN series contributes at its finite steps, so it
        // counts (the per-step flicker is the documented scalar residual).
        let partial = [f64::NAN, 1.0];
        let links = [
            RelLinkInput {
                to: "z",
                score: Some(&finite),
            },
            RelLinkInput {
                to: "z",
                score: Some(&all_nan),
            },
            RelLinkInput {
                to: "y",
                score: Some(&finite),
            },
            RelLinkInput {
                to: "y",
                score: Some(&partial),
            },
        ];
        assert_eq!(rel_link_group_sizes(&links), vec![1, 0, 2, 2]);
    }

    // --- Relative link scores (GH #652) ---------------------------------

    #[test]
    fn rel_link_two_inputs_into_one_target_normalizes_per_step() {
        // Two links into the same target `z` with known raw scores; the
        // relative score is score_i / (|s1| + |s2|) at each step.
        let s1 = [2.0, -3.0, 0.0];
        let s2 = [6.0, 1.0, 5.0];
        let links = [
            RelLinkInput {
                to: "z",
                score: Some(&s1),
            },
            RelLinkInput {
                to: "z",
                score: Some(&s2),
            },
        ];
        let out = compute_rel_link_scores(&links, 3);
        let r1 = out[0].as_ref().unwrap();
        let r2 = out[1].as_ref().unwrap();
        // step 0: denom = 8; step 1: denom = 4; step 2: denom = 5
        assert_eq!(r1, &[2.0 / 8.0, -3.0 / 4.0, 0.0 / 5.0]);
        assert_eq!(r2, &[6.0 / 8.0, 1.0 / 4.0, 5.0 / 5.0]);
        // Signs are preserved (signed, not absolute), and the per-step
        // magnitudes of siblings into the same target sum to 1.
        for t in 0..3 {
            assert!((r1[t].abs() + r2[t].abs() - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn rel_link_unscored_links_get_none_and_dont_contribute() {
        // A `None`-score link (constant/parameter, or out-of-loop in
        // exhaustive mode) gets `None` back and is excluded from the
        // denominator of its target's scored siblings.
        let s = [4.0, 4.0];
        let links = [
            RelLinkInput {
                to: "z",
                score: Some(&s),
            },
            RelLinkInput {
                to: "z",
                score: None,
            },
        ];
        let out = compute_rel_link_scores(&links, 2);
        // The scored link normalizes only against itself -> +1 every step.
        assert_eq!(out[0].as_ref().unwrap(), &[1.0, 1.0]);
        assert!(out[1].is_none());
    }

    #[test]
    fn rel_link_separate_targets_normalize_independently() {
        // Links into different targets do NOT cross-normalize: each target
        // is its own denominator group.  This is the cross-target
        // comparability the raw score lacks.
        let a = [10.0];
        let b = [20.0];
        let links = [
            RelLinkInput {
                to: "y",
                score: Some(&a),
            },
            RelLinkInput {
                to: "z",
                score: Some(&b),
            },
        ];
        let out = compute_rel_link_scores(&links, 1);
        // Each is the lone scored input of its own target -> +1.
        assert_eq!(out[0].as_ref().unwrap(), &[1.0]);
        assert_eq!(out[1].as_ref().unwrap(), &[1.0]);
    }

    #[test]
    fn rel_link_nan_summand_excluded_siblings_use_healthy_denom() {
        // One input's score is NaN at step 0; its sibling must still
        // normalize against the healthy denominator (NaN excluded), and the
        // NaN link's own relative score stays NaN at that step.
        let s_bad = [f64::NAN, 3.0];
        let s_ok = [4.0, 1.0];
        let links = [
            RelLinkInput {
                to: "z",
                score: Some(&s_bad),
            },
            RelLinkInput {
                to: "z",
                score: Some(&s_ok),
            },
        ];
        let out = compute_rel_link_scores(&links, 2);
        let r_bad = out[0].as_ref().unwrap();
        let r_ok = out[1].as_ref().unwrap();
        // step 0: denom excludes the NaN -> 4; healthy sibling = 4/4 = 1.
        assert_eq!(r_ok[0], 1.0);
        // The NaN link's own numerator is NaN -> NaN/4 = NaN.
        assert!(r_bad[0].is_nan());
        // step 1: denom = |3| + |1| = 4; both well-defined.
        assert_eq!(r_bad[1], 3.0 / 4.0);
        assert_eq!(r_ok[1], 1.0 / 4.0);
    }

    #[test]
    fn rel_link_inf_summand_kept_dominant_link_diverges() {
        // An Inf input dominates its target: the finite sibling normalizes
        // to 0 (finite/Inf) and the Inf link itself to NaN (Inf/Inf),
        // matching the loop-level Inf-retention semantics.
        let s_inf = [f64::INFINITY];
        let s_fin = [5.0];
        let links = [
            RelLinkInput {
                to: "z",
                score: Some(&s_inf),
            },
            RelLinkInput {
                to: "z",
                score: Some(&s_fin),
            },
        ];
        let out = compute_rel_link_scores(&links, 1);
        assert!(out[0].as_ref().unwrap()[0].is_nan());
        assert_eq!(out[1].as_ref().unwrap()[0], 0.0);
    }

    #[test]
    fn rel_link_zero_denominator_yields_zero() {
        // When every input into a target is 0 at a step, the denominator is
        // 0 and SAFEDIV-0 yields 0 (not NaN).
        let s1 = [0.0, 2.0];
        let s2 = [0.0, 2.0];
        let links = [
            RelLinkInput {
                to: "z",
                score: Some(&s1),
            },
            RelLinkInput {
                to: "z",
                score: Some(&s2),
            },
        ];
        let out = compute_rel_link_scores(&links, 2);
        assert_eq!(out[0].as_ref().unwrap()[0], 0.0);
        assert_eq!(out[1].as_ref().unwrap()[0], 0.0);
        assert_eq!(out[0].as_ref().unwrap()[1], 0.5);
    }

    #[test]
    fn rel_link_degenerate_huge_raw_ranks_below_active_target() {
        // The GH #652 scenario in miniature.  `near_const` is a target that
        // barely moves: its two inputs have astronomically large RAW scores
        // (1e22-scale), because the raw denominator (Δtarget) is tiny.  An
        // `active` target moves normally: its dominant input has a modest raw
        // score (~0.9).  Ranking by mean |raw| puts the degenerate links on
        // top; ranking by mean |relative| must put the genuinely-dominant
        // link of the active target above the degenerate huge-raw link.
        let near_a = [6.4e22, 6.4e22];
        let near_b = [1.1e22, 1.1e22];
        let active_dom = [0.9, 0.9];
        let active_min = [0.1, 0.1];
        let links = [
            RelLinkInput {
                to: "near_const",
                score: Some(&near_a),
            },
            RelLinkInput {
                to: "near_const",
                score: Some(&near_b),
            },
            RelLinkInput {
                to: "active",
                score: Some(&active_dom),
            },
            RelLinkInput {
                to: "active",
                score: Some(&active_min),
            },
        ];
        let out = compute_rel_link_scores(&links, 2);

        // Mean |raw| ranking (the bad workflow) puts the degenerate links first.
        let mean_abs = |s: &[f64]| s.iter().map(|v| v.abs()).sum::<f64>() / s.len() as f64;
        let raw_rank_top = [
            mean_abs(&near_a),
            mean_abs(&near_b),
            mean_abs(&active_dom),
            mean_abs(&active_min),
        ];
        assert!(
            raw_rank_top[0] > raw_rank_top[2],
            "raw ranking degenerately puts near-constant link above the active one"
        );

        // Mean |relative| ranking (the fix): the active target's dominant
        // link (0.9 / 1.0 = 0.9) must outrank the degenerate near-constant
        // link (6.4e22 / 7.5e22 ~= 0.853).
        let rel_active_dom = mean_abs(out[2].as_ref().unwrap());
        let rel_near_a = mean_abs(out[0].as_ref().unwrap());
        assert!(
            rel_active_dom > rel_near_a,
            "relative ranking should put the active target's dominant link \
             ({rel_active_dom}) above the degenerate huge-raw link ({rel_near_a})"
        );
        // And every relative score is bounded in [-1, 1].
        for series in out.iter().flatten() {
            for &v in series {
                assert!(v.abs() <= 1.0 + 1e-12, "relative score {v} out of [-1,1]");
            }
        }
    }

    proptest! {
        /// For any target group, the per-step sum of |relative score| over
        /// the group's scored links is either 0 (all-zero / NaN-only step)
        /// or 1 (SAFEDIV partition identity), and every finite relative
        /// score lies in [-1, 1].
        #[test]
        fn rel_link_group_magnitudes_sum_to_one_or_zero(
            raws in proptest::collection::vec(-1e6f64..1e6f64, 1..6),
        ) {
            let series: Vec<[f64; 1]> = raws.iter().map(|&v| [v]).collect();
            let links: Vec<RelLinkInput> = series
                .iter()
                .map(|s| RelLinkInput { to: "z", score: Some(s.as_slice()) })
                .collect();
            let out = compute_rel_link_scores(&links, 1);
            let denom: f64 = raws.iter().map(|v| v.abs()).sum();
            let sum_abs: f64 = out
                .iter()
                .map(|o| o.as_ref().unwrap()[0].abs())
                .sum();
            if denom == 0.0 {
                prop_assert_eq!(sum_abs, 0.0);
            } else {
                prop_assert!((sum_abs - 1.0).abs() < 1e-9);
            }
            for o in &out {
                let v = o.as_ref().unwrap()[0];
                prop_assert!(v.abs() <= 1.0 + 1e-9);
            }
        }
    }
}
