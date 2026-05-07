// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Structural dimension-element recovery via the section-5 + record-field[8]
//! rules validated on `Ref.vdf`.
//!
//! Two signals combine to label array dimensions without any model input:
//!
//! 1. Record field[8] grouping pairs a dimension "anchor" record
//!    (`f[6]==0`, `f[14]==sentinel`, a valid group id in `f[8]`) with its
//!    zero-based element records (`f[6]==0`, `f[12]==124`, `f[10]==0`,
//!    `f[14]!=sentinel`, `f[11]` a contiguous 0..N index). Complete
//!    groups yield full root-dim element lists.
//! 2. Sorting anchors by `f[8]` ascending produces a 1:1 canonical
//!    ordering with section-5 entries in file order. For subrange dims
//!    (no element records of their own), the subrange's sec5 payload is
//!    an in-order subsequence of its parent root dim's payload; the
//!    subseq positions are the MDL-declared element indices.
//!
//! This module mirrors `_recover_dimension_sets` in `tools/vdf_xray.py`
//! so that Rust consumers can recover dim names + elements without
//! re-implementing the rule set per caller. See
//! `/tmp/vdf_ref_dims.md` for the numeric evidence.

use std::collections::{HashMap, HashSet};

use super::{
    STDLIB_PARTICIPANT_HELPERS, VDF_SENTINEL, VENSIM_BUILTINS, VdfDimensionSet, VdfFile,
    VdfSection5SetEntry, is_vdf_metadata_entry, read_u16,
};

/// Anchor-level dimension facts decoded from the record stream.
///
/// A dim is "complete" when its element records form a contiguous 0..N
/// run with the expected cardinality. Incomplete anchors (subrange
/// anchors with no element records of their own, or dims with missing
/// run-time entries) carry an empty element list and still contribute a
/// stable anchor-name / group-id pair that pairs 1:1 with a section-5
/// entry.
#[derive(Debug, Clone)]
struct Anchor {
    /// Anchor record's name as recovered from the section-2 name table.
    name: String,
    /// Index into `VdfFile::records` for the anchor record.
    record_index: usize,
    /// `f[8]` group id shared by the anchor and its element records.
    group_id: u32,
    /// Zero-based element records sorted by `f[11]` ascending. Empty for
    /// subrange dims whose elements are encoded via the sec5-subseq rule.
    elements: Vec<(u32, String)>,
    /// True when `elements` forms a contiguous 0..N run of length >= 2
    /// (matching the Python decoder's `status == "complete"`).
    complete: bool,
}

/// Replay the section-2 name-table layout to map every record `field[2]`
/// name key to its printable name. This duplicates the private
/// `VdfFile::record_name_key_to_name_index` so the recovery path does
/// not depend on additional private accessors.
fn build_name_key_to_name(vdf: &VdfFile) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    let Some(name_section_idx) = vdf.name_section_idx else {
        return out;
    };
    let Some(section) = vdf.sections.get(name_section_idx) else {
        return out;
    };
    if vdf.names.is_empty() {
        return out;
    }
    let data_start = section.data_offset();
    let parse_end = section.region_end.min(vdf.data.len());
    let first_len = (section.field5 >> 16) as usize;
    if first_len == 0 || data_start + first_len > vdf.data.len() {
        return out;
    }
    // The first name has no length prefix and its canonical key is 7.
    out.insert(7u32, vdf.names[0].clone());
    let mut pos = data_start + first_len;
    let mut name_idx = 1usize;
    while name_idx < vdf.names.len() && pos + 2 <= parse_end {
        let len = read_u16(&vdf.data, pos) as usize;
        pos += 2;
        if len == 0 {
            continue;
        }
        if pos + len > parse_end || len > 256 {
            break;
        }
        let start_rel = pos - data_start;
        if start_rel.is_multiple_of(4) {
            out.insert((start_rel / 4 + 7) as u32, vdf.names[name_idx].clone());
        }
        pos += len;
        name_idx += 1;
    }
    out
}

fn is_visible_model_name(name: &str) -> bool {
    if name.is_empty() || is_vdf_metadata_entry(name) {
        return false;
    }
    if STDLIB_PARTICIPANT_HELPERS.contains(&name) {
        return false;
    }
    if VENSIM_BUILTINS
        .iter()
        .any(|builtin| builtin.eq_ignore_ascii_case(name))
    {
        return false;
    }
    true
}

fn valid_dimension_group_id(group_id: u32) -> bool {
    group_id != 0 && group_id != VDF_SENTINEL
}

/// Recover dimension anchors from the record stream.
///
/// Returns one anchor per `(group_id, anchor-name)` pair whose group has
/// at least one anchor or element record. The returned order is ascending
/// `group_id`, with lexicographic tie-break on the anchor name.
fn recover_anchors(vdf: &VdfFile) -> Vec<Anchor> {
    let key_to_name = build_name_key_to_name(vdf);
    if key_to_name.is_empty() {
        return Vec::new();
    }

    let mut candidate_anchors: HashMap<u32, Vec<(usize, String)>> = HashMap::new();
    let mut element_groups: HashMap<u32, Vec<(u32, String)>> = HashMap::new();

    for (rec_idx, rec) in vdf.records.iter().enumerate() {
        if rec.fields[6] == 0 {
            let group_id = rec.fields[8];
            if !valid_dimension_group_id(group_id) {
                continue;
            }
            let Some(name) = key_to_name.get(&rec.fields[2]) else {
                continue;
            };
            if !is_visible_model_name(name) {
                continue;
            }

            if rec.fields[12] == 124
                && rec.fields[10] == 0
                && rec.fields[14] != VDF_SENTINEL
                && rec.fields[11] < 4096
            {
                element_groups
                    .entry(group_id)
                    .or_default()
                    .push((rec.fields[11], name.clone()));
                continue;
            }
            if rec.fields[14] == VDF_SENTINEL {
                candidate_anchors
                    .entry(group_id)
                    .or_default()
                    .push((rec_idx, name.clone()));
            }
            continue;
        }

        let alt_group_id = rec.fields[12];
        let Some(name) = key_to_name.get(&rec.fields[6]) else {
            continue;
        };
        if valid_dimension_group_id(alt_group_id)
            && rec.fields[10] == 0
            && rec.fields[11] == 0
            && rec.fields[13] == VDF_SENTINEL
            && rec.fields[14] != VDF_SENTINEL
            && rec.fields[15] < 4096
            && is_visible_model_name(name)
        {
            element_groups
                .entry(alt_group_id)
                .or_default()
                .push((rec.fields[15], name.clone()));
        }
    }

    let mut anchors: Vec<Anchor> = Vec::new();
    let all_groups: HashSet<u32> = candidate_anchors
        .keys()
        .chain(element_groups.keys())
        .copied()
        .collect();
    for group_id in all_groups {
        let candidates = candidate_anchors
            .get(&group_id)
            .cloned()
            .unwrap_or_default();
        let raw_elements = element_groups.get(&group_id).cloned().unwrap_or_default();

        // Deduplicate element indices, preferring the first name seen
        // per index (matches Python's `by_index` insertion order).
        let mut by_index: HashMap<u32, String> = HashMap::new();
        for (idx, ename) in raw_elements {
            by_index.entry(idx).or_insert(ename);
        }
        let mut ordered_indices: Vec<u32> = by_index.keys().copied().collect();
        ordered_indices.sort_unstable();
        let expected: Vec<u32> = (0..ordered_indices.len() as u32).collect();
        let complete = !ordered_indices.is_empty()
            && ordered_indices == expected
            && ordered_indices.len() >= 2;
        let elements: Vec<(u32, String)> = ordered_indices
            .iter()
            .map(|i| (*i, by_index.remove(i).unwrap_or_default()))
            .collect();

        // Deduplicate anchors by lowercased name and emit one entry per
        // distinct anchor-name in the group. Subrange anchors carry the
        // same elements list via the sec5-subseq binding.
        let mut seen_names: HashSet<String> = HashSet::new();
        let mut sorted_candidates = candidates.clone();
        sorted_candidates.sort_by_key(|(rec_idx, name)| (*rec_idx, name.clone()));
        for (record_index, name) in &sorted_candidates {
            let key = name.to_lowercase();
            if !seen_names.insert(key) {
                continue;
            }
            anchors.push(Anchor {
                name: name.clone(),
                record_index: *record_index,
                group_id,
                elements: elements.clone(),
                complete,
            });
        }
    }

    anchors.sort_by(|a, b| {
        a.group_id
            .cmp(&b.group_id)
            .then_with(|| a.name.cmp(&b.name))
    });
    anchors
}

pub(super) fn section3_axis_ref_to_dimension_name(vdf: &VdfFile) -> HashMap<u32, String> {
    let Some(sec1) = vdf.sections.get(1) else {
        return HashMap::new();
    };
    let sec1_data_offset = sec1.data_offset();
    let mut out = HashMap::new();

    for anchor in recover_anchors(vdf) {
        let Some(record) = vdf.records.get(anchor.record_index) else {
            continue;
        };
        let field9_offset = record.file_offset + 9 * 4;
        let Some(rel) = field9_offset.checked_sub(sec1_data_offset) else {
            continue;
        };
        if rel.is_multiple_of(4) {
            out.entry((rel / 4) as u32).or_insert(anchor.name);
        }
    }

    out
}

fn sec5_payload(entry: &VdfSection5SetEntry) -> &[u32] {
    let n = entry.n.min(entry.refs.len());
    &entry.refs[..n]
}

/// Return the subsequence positions of `needle` inside `hay` if
/// `needle` is a strict in-order subsequence of `hay`; otherwise `None`.
fn subseq_positions(needle: &[u32], hay: &[u32]) -> Option<Vec<usize>> {
    let mut positions = Vec::with_capacity(needle.len());
    let mut i = 0usize;
    for (j, &h) in hay.iter().enumerate() {
        if i < needle.len() && needle[i] == h {
            positions.push(j);
            i += 1;
        }
    }
    if i == needle.len() {
        Some(positions)
    } else {
        None
    }
}

/// Mirror `_recover_dimension_sets` from the Python xray tool.
///
/// Walks the record stream plus the section-5 set stream and returns one
/// `VdfDimensionSet` per decoded dimension. Complete root dims come
/// from their `f[8]` element groups directly. Subrange dims are
/// recovered by matching their sec5 payload as an in-order subsequence
/// of a root dim's payload: the subseq positions index into the root's
/// element names.
///
/// Returns an empty vector when no anchor group carries enough signal
/// (fixtures without arrayed owners degenerate cleanly).
pub(super) fn recover_dimension_sets_via_sec5(vdf: &VdfFile) -> Vec<VdfDimensionSet> {
    let anchors = recover_anchors(vdf);
    if anchors.is_empty() {
        return Vec::new();
    }
    let sec5 = match vdf.parse_section5_set_stream() {
        Some((_, entries, _)) => entries,
        None => return Vec::new(),
    };
    // Anchor/sec5 pairing rule requires a 1:1 match. If the counts
    // disagree, the fixture does not satisfy the pinned alignment rule
    // and we bail out rather than fabricate a partial recovery.
    if anchors.len() != sec5.len() {
        return Vec::new();
    }

    // Collect payloads in the same canonical order (f[8]-ascending).
    let payloads: Vec<Vec<u32>> = sec5.iter().map(|e| sec5_payload(e).to_vec()).collect();

    // Two-pass recovery: first emit complete root dims with their own
    // element records, then resolve subrange dims via payload-subsequence
    // matching against the already-resolved roots.
    let mut results: HashMap<String, VdfDimensionSet> = HashMap::new();
    for (i, anchor) in anchors.iter().enumerate() {
        if !anchor.complete {
            continue;
        }
        // A root dim's payload must not be an in-order subsequence of
        // any strictly-longer payload (otherwise it would itself be a
        // subrange of another dim). This matches the Python "root_ok"
        // check in the recovery snippet.
        let is_root = anchors.iter().enumerate().all(|(j, _)| {
            j == i
                || payloads[i].len() >= payloads[j].len()
                || subseq_positions(&payloads[i], &payloads[j]).is_none()
        });
        if !is_root {
            continue;
        }
        let elements: Vec<String> = anchor
            .elements
            .iter()
            .map(|(_, name)| name.clone())
            .collect();
        results.insert(
            anchor.name.clone(),
            VdfDimensionSet {
                name: anchor.name.clone(),
                elements,
            },
        );
    }

    for (i, anchor) in anchors.iter().enumerate() {
        if results.contains_key(&anchor.name) {
            continue;
        }
        // Find the unique already-resolved root dim whose payload
        // contains this anchor's payload as a subsequence.
        let mut best: Option<(usize, Vec<usize>, String)> = None;
        for (j, parent) in anchors.iter().enumerate() {
            if i == j {
                continue;
            }
            let Some(parent_set) = results.get(&parent.name) else {
                continue;
            };
            let Some(positions) = subseq_positions(&payloads[i], &payloads[j]) else {
                continue;
            };
            if positions.len() != payloads[i].len() {
                continue;
            }
            if parent_set.elements.len() != payloads[j].len() {
                continue;
            }
            match best {
                None => {
                    best = Some((j, positions, parent.name.clone()));
                }
                Some((_, _, ref existing_name)) => {
                    // Prefer the parent whose name sorts first so the
                    // result is deterministic when multiple roots admit
                    // the subsequence (this matches the Python "prefer
                    // root dim over another subrange as parent").
                    if parent.name.as_str() < existing_name.as_str() {
                        best = Some((j, positions.clone(), parent.name.clone()));
                    }
                }
            }
        }
        if let Some((_, positions, parent_name)) = best
            && let Some(parent_set) = results.get(&parent_name)
        {
            let elements: Vec<String> = positions
                .iter()
                .filter_map(|&k| parent_set.elements.get(k).cloned())
                .collect();
            if elements.len() == payloads[i].len() {
                results.insert(
                    anchor.name.clone(),
                    VdfDimensionSet {
                        name: anchor.name.clone(),
                        elements,
                    },
                );
            }
        }
    }

    // Emit results in anchor order for stability across runs.
    anchors
        .iter()
        .filter_map(|a| results.remove(&a.name))
        .collect()
}
