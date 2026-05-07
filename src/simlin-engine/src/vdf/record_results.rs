// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Record-derived result owner selection.
//!
//! Section-1 records can contain owner-like OT spans that overlap when a
//! descriptor points into the same runtime array as a real saved variable.
//! This module keeps the selection logic out of `vdf.rs` and models the
//! invariant expected by extracted time-series data: emitted owner spans form a
//! non-overlapping OT partition. The overlap selector is reconstruction, not a
//! decoded Vensim owner/descriptor flag.

use std::collections::{HashMap, HashSet};

use super::{VdfFile, VdfSection3Directory, VdfSection3DirectoryEntry};

#[derive(Clone, Debug)]
pub(super) struct RecordResultCandidate {
    pub(super) start: usize,
    pub(super) span: usize,
    pub(super) sort_rank: usize,
    pub(super) first_name_key: u32,
    pub(super) record_indices: Vec<usize>,
}

impl RecordResultCandidate {
    pub(super) fn end(&self) -> usize {
        self.start + self.span
    }
}

/// Keep the largest non-overlapping set of record-derived result spans.
///
/// Equal-coverage choices prefer lower record sort keys, which matches the
/// observed `Ref.vdf` descriptor conflicts where real owner records sort near
/// their local variable group and descriptor records sort far away. This is a
/// conservative extraction rule while the direct on-disk discriminator remains
/// unknown.
pub(super) fn select_non_overlapping_record_candidates(
    mut candidates: Vec<RecordResultCandidate>,
) -> Vec<RecordResultCandidate> {
    if candidates.len() <= 1 {
        return candidates;
    }

    candidates.sort_by_key(|candidate| (candidate.end(), candidate.start));
    let ends: Vec<usize> = candidates.iter().map(RecordResultCandidate::end).collect();
    let mut dp: Vec<((usize, i64, usize), Vec<usize>)> = vec![((0, 0, 0), Vec::new())];

    for (idx, candidate) in candidates.iter().enumerate() {
        let compatible_count = ends.partition_point(|&end| end <= candidate.start);
        let (prev_score, prev_selection) = &dp[compatible_count];
        let rank = if candidate.sort_rank == usize::MAX {
            1_000_000_000i64
        } else {
            candidate.sort_rank as i64
        };
        let weight = (candidate.span, -rank, 1usize);
        let include_score = (
            prev_score.0 + weight.0,
            prev_score.1 + weight.1,
            prev_score.2 + weight.2,
        );
        let mut include_selection = prev_selection.clone();
        include_selection.push(idx);

        let (exclude_score, exclude_selection) = dp.last().expect("dp has seed");
        if include_score > *exclude_score {
            dp.push((include_score, include_selection));
        } else {
            dp.push((*exclude_score, exclude_selection.clone()));
        }
    }

    let selected_indices: HashSet<usize> = dp
        .last()
        .map(|(_, v)| v.iter().copied().collect())
        .unwrap_or_default();
    let mut selected_candidates: Vec<RecordResultCandidate> = candidates
        .into_iter()
        .enumerate()
        .filter_map(|(idx, candidate)| selected_indices.contains(&idx).then_some(candidate))
        .collect();
    selected_candidates.sort_by_key(|candidate| candidate.first_name_key);
    selected_candidates
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
/// The candidate has already established the base variable and OT span. This
/// step is deliberately narrower: it only emits element labels when the span's
/// section-3 shape resolves to axis refs that point at decoded dimension
/// anchors with matching cardinalities. Otherwise callers keep the old numeric
/// fallback rather than guessing from same-size dimensions.
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
