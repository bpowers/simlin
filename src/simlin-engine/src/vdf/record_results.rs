// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Record-derived result extraction.
//!
//! Section 1 of a VDF stores one record per name-bound symbol, and a record's
//! `f[11]` doubles as either an OT-block start (owner) or a section-6
//! lookup-record index (graphical-function descriptor) -- an *untagged* union
//! whose discriminator is not stored on disk (see `docs/design/vdf.md`
//! appendix). The reader is expected to already know the model. For the
//! model-free reader this module reconstructs which records are owners using
//! the decoded forward link: a descriptor's `f[11]` indexes the section-6
//! lookup-record array, and that array is in case-insensitive alphabetical
//! order of the lookup-definition names.
//!
//! `decoded_record_spans` produces one `DecodedRecordSpan` per section-1
//! record whose `(name key, OT-start, shape)` triple is structurally valid
//! and whose covered OT slots all carry an owner class code.
//! `identify_descriptor_records` then peels off the descriptor records that
//! collide with real owner spans, leaving a clean non-overlapping owner
//! partition (`Time` at OT[0] aside).

use std::collections::{HashMap, HashSet};

use super::{
    SYSTEM_NAMES, VdfFile, VdfRecord, VdfSection3Directory, VdfSection3DirectoryEntry,
    is_lookupish_name, is_owner_ot_class_code,
};
use crate::common::{Canonical, Ident};

/// One direct record -> name -> OT-span fact.
///
/// Built by `decoded_record_spans`. A span here means the record carries:
/// - an `f[2]` that resolves through the section-2 name-key formula;
/// - an `f[11]` interpretable as an OT block start in `[1, ot_count)`;
/// - a non-zero `f[6]` shape code whose flat span is structurally decoded;
/// - and (class-code guard) an OT slot at `f[11]` whose section-6 class
///   code marks real saved data (`is_owner_ot_class_code`).
///
/// Whether the record is the *emitted* series owner is a separate question
/// answered by `identify_descriptor_records`.
#[derive(Clone, Debug)]
pub(super) struct DecodedRecordSpan {
    pub(super) rec_idx: usize,
    pub(super) name: String,
    pub(super) start: usize,
    pub(super) end: usize,
    /// `f[10]`, used as the descriptor tie-break when the lexical
    /// lookup-def name test is ambiguous.
    pub(super) sort_key: u32,
}

impl DecodedRecordSpan {
    pub(super) fn length(&self) -> usize {
        self.end - self.start
    }
}

/// Compute the structural OT-flat span of a record, returning `None` when
/// the shape cannot be resolved.
///
/// `f[6] == 5` is the scalar marker (one slot). `f[6] == 32` is Vensim's
/// generic single-shape arrayed marker; it binds when exactly one
/// section-3 entry has a non-zero flat size. Otherwise the section-3
/// directory's per-shape-code entry is used.
fn decoded_record_shape_length(
    rec: &VdfRecord,
    section3_directory: Option<&VdfSection3Directory>,
    sec3_sole_flat_size: Option<usize>,
) -> Option<usize> {
    let shape_code = rec.fields[6];
    if shape_code == 0 {
        return None;
    }
    if shape_code == 5 {
        return Some(1);
    }
    if let Some(dir) = section3_directory
        && let Some(entry) = dir.entry_for_record_shape_code(shape_code)
    {
        let s = entry.flat_size();
        if s >= 1 {
            return Some(s);
        }
    }
    if shape_code == 32
        && let Some(s) = sec3_sole_flat_size
        && s >= 1
    {
        return Some(s);
    }
    None
}

/// Build the direct record -> name -> OT-span facts from a VDF.
///
/// This deliberately performs no descriptor pruning, no owner selection, no
/// name-category filtering, and no array-label guessing. Whether a span is
/// the user-facing series owner is decided downstream in
/// `identify_descriptor_records`, which is the only place that resolves the
/// `f[11]` owner/descriptor union.
pub(super) fn decoded_record_spans(
    vdf: &VdfFile,
    name_key_to_name_index: &HashMap<u32, usize>,
    section3_directory: Option<&VdfSection3Directory>,
) -> Vec<DecodedRecordSpan> {
    let codes = vdf.section6_ot_class_codes();
    let sec3_sole_flat_size = section3_directory.and_then(|d| {
        let sizes: HashSet<usize> = d
            .entries
            .iter()
            .map(|e| e.flat_size())
            .filter(|&s| s > 0)
            .collect();
        if sizes.len() == 1 {
            sizes.into_iter().next()
        } else {
            None
        }
    });

    let mut spans = Vec::new();
    for (rec_idx, rec) in vdf.records.iter().enumerate() {
        let Some(&name_idx) = name_key_to_name_index.get(&rec.fields[2]) else {
            continue;
        };
        let Some(name) = vdf.names.get(name_idx).cloned() else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let start = rec.fields[11] as usize;
        if start == 0 || start >= vdf.offset_table_count {
            continue;
        }
        let length = match decoded_record_shape_length(rec, section3_directory, sec3_sole_flat_size)
        {
            Some(l) => l,
            None => continue,
        };
        let end = start + length;
        if end > vdf.offset_table_count {
            continue;
        }
        // Class-code guard: every OT slot in the span must carry a real-data
        // owner code. Time (0x0f) is excluded by `start >= 1`; any non-owner
        // code in-range indicates a descriptor reinterpretation of `f[11]`
        // or a stale ghost record, not a real owner span.
        if let Some(ref code_vec) = codes {
            let mut all_owner = true;
            for ot_idx in start..end {
                let code = code_vec.get(ot_idx).copied().unwrap_or(0);
                if !is_owner_ot_class_code(code) {
                    all_owner = false;
                    break;
                }
            }
            if !all_owner {
                continue;
            }
        }

        spans.push(DecodedRecordSpan {
            rec_idx,
            name,
            start,
            end,
            sort_key: rec.fields[10],
        });
    }
    spans
}

/// Result of identifying graphical-function descriptor records.
///
/// `used_f10_fallback` records when the descriptor peeling step had to
/// resort to the highest-`f[10]` tie-break because the lexical
/// lookup-def-name test was ambiguous (`Ref.vdf` is the canonical case).
/// The flag is exposed for tests and future diagnostics; it has no effect
/// on the descriptor membership decision itself.
#[derive(Clone, Debug, Default)]
pub(super) struct DescriptorIdentification {
    pub(super) descriptor_indices: HashSet<usize>,
    #[allow(dead_code)]
    pub(super) used_f10_fallback: bool,
}

/// Iterative union-find without rank.
struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, x: usize, y: usize) {
        let px = self.find(x);
        let py = self.find(y);
        if px != py {
            self.parent[px] = py;
        }
    }
}

/// Identify graphical-function descriptor records among the decoded spans.
///
/// Background. Vensim stores graphical-function definitions ("descriptor"
/// records) and their consuming variables ("owner" records) side-by-side in
/// section 1 with `f[11]` as an *untagged* union: for owners it is the
/// OT-block start, for descriptors it is the zero-based index into the
/// section-6 lookup-record array (case-insensitive alphabetical order of
/// the lookup-def names). The on-disk format does not store the
/// discriminator -- a field-by-field analysis (vdf.md appendix "Claims
/// about the owner/descriptor discriminator") confirms no byte, bit, or
/// `(f0, f1)` combination distinguishes the two.
///
/// Algorithm. Spans that overlap in OT space form a connected component
/// (descriptors sometimes have arrayed shapes that cross owner ranges, so
/// they need not literally share `f[11]` with their colliding owners).
/// Within each component, peel off descriptor records iteratively:
/// 1. **Lookup-def name test.** If exactly one candidate's name is
///    lexically lookupish (`is_lookupish_name`) it is the descriptor.
/// 2. **Highest-`f[10]` fallback.** When the lookup-def name test is
///    ambiguous (e.g. `Ref.vdf` where descriptors are domain
///    abbreviations), the candidate with the highest `f[10]` is treated as
///    the descriptor and `used_f10_fallback` is flagged.
///
/// Once a record is identified as a descriptor, its true binding is the
/// decoded forward link: `lookup_record[f[11]].word[10]` is the
/// evaluated-output OT, `word[5..6]` are the section-7 x/y array offsets,
/// `word[12]` is the optional dependency-chain root.
pub(super) fn identify_descriptor_records(
    vdf: &VdfFile,
    spans: &[DecodedRecordSpan],
) -> DescriptorIdentification {
    let n_lookups = vdf.section6_lookup_records().map(|v| v.len()).unwrap_or(0);
    if n_lookups == 0 || spans.is_empty() {
        return DescriptorIdentification::default();
    }

    // Build OT-slot -> spans-claiming-it. Spans that share any OT slot with
    // another span are descriptor-pair candidates.
    let mut by_slot: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, span) in spans.iter().enumerate() {
        for ot in span.start..span.end {
            by_slot.entry(ot).or_default().push(i);
        }
    }

    // Connected components of overlapping spans (union-find on span indices).
    let mut uf = UnionFind::new(spans.len());
    for slot_spans in by_slot.values() {
        if slot_spans.len() >= 2 {
            let base = slot_spans[0];
            for &other in &slot_spans[1..] {
                uf.union(base, other);
            }
        }
    }

    // A span participates in overlap iff some OT in its range has 2+
    // claimants.
    let mut overlapping: HashSet<usize> = HashSet::new();
    for slot_spans in by_slot.values() {
        if slot_spans.len() >= 2 {
            overlapping.extend(slot_spans.iter().copied());
        }
    }

    let mut components: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, _) in spans.iter().enumerate() {
        if overlapping.contains(&i) {
            let root = uf.find(i);
            components.entry(root).or_default().push(i);
        }
    }

    let mut descriptor_indices: HashSet<usize> = HashSet::new();
    let mut used_f10_fallback = false;

    for component in components.values() {
        // Iteratively peel off descriptor records until the component is
        // internally non-overlapping. Candidates are restricted to records
        // whose `f[11]` is in `[0, lookup_count)` -- the structural
        // pre-condition for the lookup-record forward link.
        let mut active: Vec<usize> = component.clone();
        loop {
            let mut comp_by_slot: HashMap<usize, Vec<usize>> = HashMap::new();
            for &i in &active {
                let span = &spans[i];
                for ot in span.start..span.end {
                    comp_by_slot.entry(ot).or_default().push(i);
                }
            }
            let mut still_overlapping: HashSet<usize> = HashSet::new();
            for slot_spans in comp_by_slot.values() {
                if slot_spans.len() >= 2 {
                    still_overlapping.extend(slot_spans.iter().copied());
                }
            }
            if still_overlapping.is_empty() {
                break;
            }
            let candidates: Vec<usize> = active
                .iter()
                .copied()
                .filter(|&i| {
                    if !still_overlapping.contains(&i) {
                        return false;
                    }
                    let f11 = vdf.records[spans[i].rec_idx].fields[11] as usize;
                    f11 < n_lookups
                })
                .collect();
            if candidates.is_empty() {
                // Owner-only overlap with no descriptor candidate: leave the
                // component alone. The caller (or precision report) is
                // expected to surface a residual `record-span-overlap`.
                break;
            }
            let lookupish: Vec<usize> = candidates
                .iter()
                .copied()
                .filter(|&i| is_lookupish_name(&spans[i].name))
                .collect();
            let descriptor_span_idx = if lookupish.len() == 1 {
                lookupish[0]
            } else {
                used_f10_fallback = true;
                *candidates
                    .iter()
                    .max_by_key(|&&i| (spans[i].sort_key, spans[i].rec_idx))
                    .expect("candidates non-empty")
            };
            descriptor_indices.insert(spans[descriptor_span_idx].rec_idx);
            active.retain(|&i| i != descriptor_span_idx);
        }
    }

    DescriptorIdentification {
        descriptor_indices,
        used_f10_fallback,
    }
}

/// A reconstructed result-emission candidate. A `RecordResultCandidate`
/// covers exactly one OT-aligned span and binds it to one or more section-1
/// records. Multiple records can collapse onto the same span when several
/// alias names share a slot (e.g. SMOOTH/DELAY internal helpers).
#[derive(Clone, Debug)]
pub(super) struct RecordResultCandidate {
    pub(super) start: usize,
    pub(super) span: usize,
    pub(super) record_indices: Vec<usize>,
}

fn shape_template_entry_for_record_candidate<'a>(
    vdf: &VdfFile,
    candidate: &RecordResultCandidate,
    section3_directory: Option<&'a VdfSection3Directory>,
) -> Option<&'a VdfSection3DirectoryEntry> {
    let directory = section3_directory?;
    let mut by_offset: HashMap<usize, &VdfSection3DirectoryEntry> = HashMap::new();
    let mut saw_generic_array_marker = false;

    for &record_index in &candidate.record_indices {
        let Some(record) = vdf.records.get(record_index) else {
            continue;
        };
        let shape_code = record.fields[6];
        saw_generic_array_marker |= shape_code == 32;
        if shape_code == 0 || shape_code == 5 {
            continue;
        }
        if let Some(entry) = directory.entry_for_record_shape_code(shape_code)
            && entry.flat_size() == candidate.span
        {
            by_offset.insert(entry.file_offset, entry);
        }
    }

    // The generic 32 marker is only safe when the candidate's flat size
    // identifies exactly one active section-3 template.
    if by_offset.is_empty() && saw_generic_array_marker {
        let active: Vec<&VdfSection3DirectoryEntry> = directory
            .entries
            .iter()
            .filter(|entry| entry.flat_size() == candidate.span && entry.flat_size() > 0)
            .collect();
        if active.len() == 1 {
            by_offset.insert(active[0].file_offset, active[0]);
        }
    }

    if by_offset.len() == 1 {
        by_offset.into_values().next()
    } else {
        None
    }
}

/// Label an array owner span from the section-3 axis-ref bridge.
///
/// The candidate has already established the base variable and OT span.
/// This step is deliberately narrower: it only emits element labels when
/// the span's section-3 shape resolves to axis refs that point at decoded
/// dimension anchors with matching cardinalities. Otherwise callers keep
/// the old numeric fallback rather than guessing from same-size dimensions.
pub(super) fn array_element_labels_for_record_candidate(
    vdf: &VdfFile,
    candidate: &RecordResultCandidate,
    section3_directory: Option<&VdfSection3Directory>,
    dimension_elements_by_name: &HashMap<String, Vec<String>>,
    axis_ref_to_dim_name: &HashMap<u32, String>,
) -> Option<Vec<String>> {
    if candidate.span <= 1 {
        return None;
    }
    let entry = shape_template_entry_for_record_candidate(vdf, candidate, section3_directory)?;
    if entry.flat_size() != candidate.span {
        return None;
    }

    let axis_sizes = entry.axis_sizes();
    let axis_refs: Vec<u32> = entry
        .axis_slot_refs()
        .into_iter()
        .filter(|&axis_ref| axis_ref > 0)
        .collect();
    if axis_sizes.is_empty() || axis_sizes.len() != axis_refs.len() {
        return None;
    }
    let flat_size = axis_sizes
        .iter()
        .try_fold(1usize, |acc, size| acc.checked_mul(*size))?;
    if flat_size != candidate.span {
        return None;
    }

    let mut axes = Vec::with_capacity(axis_sizes.len());
    for (axis_size, axis_ref) in axis_sizes.into_iter().zip(axis_refs) {
        let dim_name = axis_ref_to_dim_name.get(&axis_ref)?;
        let elements = dimension_elements_by_name.get(&dim_name.to_lowercase())?;
        if elements.len() != axis_size {
            return None;
        }
        axes.push(elements.clone());
    }

    Some(cartesian_axis_labels(&axes))
}

fn cartesian_axis_labels(axes: &[Vec<String>]) -> Vec<String> {
    match axes {
        [] => Vec::new(),
        [single] => single.clone(),
        _ => {
            let mut labels = vec![String::new()];
            for axis in axes {
                let mut next = Vec::with_capacity(labels.len() * axis.len());
                for prefix in &labels {
                    for element in axis {
                        if prefix.is_empty() {
                            next.push(element.clone());
                        } else {
                            next.push(format!("{prefix},{element}"));
                        }
                    }
                }
                labels = next;
            }
            labels
        }
    }
}

/// Build the ordered `(Ident, OT)` column list for `to_results_via_records`.
///
/// Pipeline:
///   1. `decoded_record_spans` produces structurally-valid record-to-OT
///      spans (post class-code guard).
///   2. `identify_descriptor_records` removes graphical-function descriptor
///      records via the decoded forward link into the section-6 lookup
///      array.
///   3. The remaining owner spans are partitioned into model vs system
///      names (Vensim's case-insensitive sort decides emission order
///      within each partition); `Time` always heads the list at OT[0].
pub(super) fn build_record_result_columns(
    vdf: &VdfFile,
    name_key_to_name_index: &HashMap<u32, usize>,
    section3_directory: Option<&VdfSection3Directory>,
    dimension_elements_by_name: &HashMap<String, Vec<String>>,
    axis_ref_to_dim_name: &HashMap<u32, String>,
) -> Vec<(Ident<Canonical>, usize)> {
    let spans = decoded_record_spans(vdf, name_key_to_name_index, section3_directory);
    let desc_id = identify_descriptor_records(vdf, &spans);

    let mut model_spans: HashMap<&str, &DecodedRecordSpan> = HashMap::new();
    let mut system_spans: HashMap<&str, &DecodedRecordSpan> = HashMap::new();
    for span in spans
        .iter()
        .filter(|s| !desc_id.descriptor_indices.contains(&s.rec_idx))
    {
        if span.name == "Time" {
            continue;
        }
        let target = if SYSTEM_NAMES.contains(&span.name.as_str()) {
            &mut system_spans
        } else {
            &mut model_spans
        };
        match target.get(span.name.as_str()) {
            Some(prev) if prev.start <= span.start => {}
            _ => {
                target.insert(span.name.as_str(), span);
            }
        }
    }

    let mut ordered: Vec<(Ident<Canonical>, usize)> =
        vec![(Ident::<Canonical>::from_str_unchecked("time"), 0)];
    let mut claimed_ot: HashSet<usize> = HashSet::new();
    claimed_ot.insert(0);

    let mut model_names: Vec<&str> = model_spans.keys().copied().collect();
    model_names.sort_by_key(|name| name.to_lowercase());
    for name in model_names {
        emit_owner_span(
            vdf,
            model_spans[name],
            section3_directory,
            dimension_elements_by_name,
            axis_ref_to_dim_name,
            &mut ordered,
            &mut claimed_ot,
        );
    }

    let mut system_names_sorted: Vec<&str> = SYSTEM_NAMES
        .iter()
        .copied()
        .filter(|n| *n != "Time")
        .collect();
    system_names_sorted.sort_by_key(|name| name.to_lowercase());
    for name in system_names_sorted {
        if let Some(span) = system_spans.get(name) {
            emit_owner_span(
                vdf,
                span,
                section3_directory,
                dimension_elements_by_name,
                axis_ref_to_dim_name,
                &mut ordered,
                &mut claimed_ot,
            );
        }
    }

    ordered
}

/// Append one owner span's columns to `ordered`, marking the OT slots in
/// `claimed_ot`. The span has already been validated as an owner record
/// (post descriptor identification). Element labels are resolved via
/// `array_element_labels_for_record_candidate`, which drives the
/// shape-template -> axis-ref -> dimension-elements bridge for arrayed
/// spans.
fn emit_owner_span(
    vdf: &VdfFile,
    span: &DecodedRecordSpan,
    section3_directory: Option<&VdfSection3Directory>,
    dimension_elements_by_name: &HashMap<String, Vec<String>>,
    axis_ref_to_dim_name: &HashMap<u32, String>,
    ordered: &mut Vec<(Ident<Canonical>, usize)>,
    claimed_ot: &mut HashSet<usize>,
) {
    let candidate = RecordResultCandidate {
        start: span.start,
        span: span.length(),
        record_indices: vec![span.rec_idx],
    };
    let element_labels = array_element_labels_for_record_candidate(
        vdf,
        &candidate,
        section3_directory,
        dimension_elements_by_name,
        axis_ref_to_dim_name,
    );
    for elem in 0..candidate.span {
        let ot = candidate.start + elem;
        if !claimed_ot.insert(ot) {
            continue;
        }
        let display = if candidate.span > 1 {
            match element_labels.as_ref().and_then(|labels| labels.get(elem)) {
                Some(label) => format!("{}[{}]", span.name, label),
                None => format!("{}[{elem}]", span.name),
            }
        } else {
            span.name.clone()
        };
        // System and user names flow through `Ident::new`, which lowercases
        // and strips spaces/underscores. `#`-prefixed internal signatures
        // (and other names with non-canonicalisable characters) use
        // `from_str_unchecked` so the raw name survives as the result
        // column key; otherwise they would collapse into an empty Ident.
        let key = if display.starts_with('#') {
            Ident::<Canonical>::from_str_unchecked(&display)
        } else {
            Ident::<Canonical>::new(&display)
        };
        ordered.push((key, ot));
    }
}
