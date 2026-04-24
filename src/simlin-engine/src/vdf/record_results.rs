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

use std::collections::HashSet;

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
