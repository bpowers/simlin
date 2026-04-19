// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! View-block decoding and file-order record-to-name OT mapping.
//!
//! Two structural signals drive the fully-VDF-native name-to-OT mapping:
//!
//! 1. **View-header marker `field[1] == 138`**.
//!    [`VdfFile::record_view_groups`] partitions records into per-view
//!    groups at each f[1]==138 boundary. On the small single-view corpus
//!    and on WRLD3 SCEN01 / experiment the f[1]==138 count matches the
//!    dot-prefix name count exactly, so the partition aligns with the
//!    dot-prefix partition of the name table.
//!    The 1:1 alignment is NOT universal:
//!    - Files edited to drop the trailing `.Control` view keep the view
//!      header record but lose the dot-prefix name (`risk2.vdf`: 2 headers
//!      vs 1 dot name).
//!    - Files re-saved from newer Vensim builds may keep a malformed
//!      view-header record with `f[6]==0` and no sentinel pair
//!      (`risk.vdf` rec[86]).
//!    - Files with sub-group names like `.Agriculture.Loop1` carry dot
//!      prefixes that do NOT get dedicated header records (`Ref.vdf`: 17
//!      headers vs 69 dot names; many dot names are sub-groups of a
//!      single parent view).
//!
//!    [`VdfFile::record_view_groups_with_diagnostics`] returns the full
//!    accounting so callers can detect and handle these cases.
//!
//! 2. **Shift-by-one link through `field[11]`**. For file-order pairs
//!    (`rec[i]`, `name[i]`) — variable records with non-dot names, view
//!    headers with dot names, traversed in parallel — each record's
//!    `field[11]` is the OT index of its *successor*'s name. Time is
//!    bound to OT[0] implicitly. [`VdfFile::to_results_via_file_order_records`]
//!    applies this rule.
//!
//!    On small single-view files this reproduces the canonical mapping;
//!    on WRLD3 it reproduces most of it but loses a handful of names via
//!    the `f[11]==0` "no OT" sentinel (documented below); on
//!    compilation-order files with sub-group dot names, arrayed
//!    variables, and dimension-element names in the name table, the
//!    pairing is approximate.
//!
//! See `docs/design/vdf.md` for the full format reverse-engineering notes.

use std::collections::HashSet;
use std::{error::Error, result::Result as StdResult};

use crate::common::{Canonical, Ident};
use crate::results::Results;

use super::VdfFile;

/// Side-information about the view-block partitioning that is useful for
/// both callers (who may need to detect a mismatched fixture) and tests
/// (which need to pin the observed counts on fixtures that are NOT 1:1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewBlockDiagnostics {
    /// Count of `field[1] == 138` records in the file.
    pub view_header_count: usize,
    /// Count of dot-prefix names in the name table.
    pub dot_prefix_name_count: usize,
    /// Headers without a matching dot-prefix name in file order. Empty
    /// when the file has 1:1 alignment or when the file has *fewer*
    /// headers than dot names.
    pub unmatched_header_rec_indices: Vec<usize>,
    /// Dot-prefix names without a matching header record in file order.
    /// Empty on 1:1 fixtures; non-empty on files like `Ref.vdf` that
    /// embed sub-group dot names without dedicated header records.
    pub unmatched_dot_name_indices: Vec<usize>,
}

impl VdfFile {
    /// Partition records into per-view groups using the `field[1] == 138`
    /// view header signal.
    ///
    /// Returns a vector of record-index groups. Group `i` contains the
    /// non-header records between the `i-1`'th and `i`'th view header
    /// (or before the first header for group 0, or after the last header
    /// for the tail). View header records themselves are **not** included
    /// in any group -- they are boundary markers, not variable records.
    ///
    /// The result has length `view_header_count + 1` in every observed
    /// fixture. For files whose header count diverges from the dot-prefix
    /// name count (see module-level docs), callers should instead use
    /// [`VdfFile::record_view_groups_with_diagnostics`] to surface the
    /// divergence.
    pub fn record_view_groups(&self) -> Vec<Vec<usize>> {
        let mut groups: Vec<Vec<usize>> = Vec::new();
        let mut current: Vec<usize> = Vec::new();
        for (idx, rec) in self.records.iter().enumerate() {
            if rec.is_view_header() {
                groups.push(std::mem::take(&mut current));
            } else {
                current.push(idx);
            }
        }
        groups.push(current);
        groups
    }

    /// Partition records into per-view groups and also report where the
    /// groups diverge from the dot-prefix name partition.
    ///
    /// This surfaces the fixtures where the 1:1 alignment from structural
    /// signal #11 breaks (re-saved/edited files and large multi-subgroup
    /// files). Callers that depend on 1:1 alignment can check
    /// `diag.unmatched_header_rec_indices.is_empty() &&
    /// diag.unmatched_dot_name_indices.is_empty()`.
    pub fn record_view_groups_with_diagnostics(&self) -> (Vec<Vec<usize>>, ViewBlockDiagnostics) {
        let groups = self.record_view_groups();
        let pairing = self.pair_headers_with_dots();
        let diag = ViewBlockDiagnostics {
            view_header_count: pairing.header_rec_indices.len(),
            dot_prefix_name_count: pairing.dot_name_indices.len(),
            unmatched_header_rec_indices: pairing.unmatched_headers,
            unmatched_dot_name_indices: pairing.unmatched_dots,
        };
        (groups, diag)
    }

    fn pair_headers_with_dots(&self) -> HeaderDotPairing {
        let header_rec_indices: Vec<usize> = self
            .records
            .iter()
            .enumerate()
            .filter_map(|(i, r)| if r.is_view_header() { Some(i) } else { None })
            .collect();
        let dot_name_indices: Vec<usize> = self
            .names
            .iter()
            .enumerate()
            .filter_map(|(i, n)| if n.starts_with('.') { Some(i) } else { None })
            .collect();
        let matched = header_rec_indices.len().min(dot_name_indices.len());
        let unmatched_headers = header_rec_indices[matched..].to_vec();
        let unmatched_dots = dot_name_indices[matched..].to_vec();
        HeaderDotPairing {
            header_rec_indices,
            dot_name_indices,
            unmatched_headers,
            unmatched_dots,
        }
    }

    /// Build a `Results` using the file-order record-to-name pairing with a
    /// shift-by-one link through `field[11]`.
    ///
    /// The pairing rule (validated against every observed scalar fixture):
    ///
    /// 1. Records are taken in file order. Records with `field[1] == 138`
    ///    are *view header* markers; all others are variable records.
    /// 2. Names are taken in file order, partitioned the same way: names
    ///    starting with `'.'` are *view* entries; the rest are variable
    ///    entries.
    /// 3. Variable records pair with variable names, view headers pair with
    ///    view entries, one-to-one in file order. This produces a stream
    ///    of `(record_idx, name_idx)` pairs. [`Self::build_file_order_pairs`]
    ///    returns the pairing plus a diagnostics record that reports any
    ///    unmatched headers or dot-prefix names so callers can detect a
    ///    divergent fixture.
    /// 4. For each adjacent pair `(pair[i], pair[i+1])`, the record
    ///    referenced by `pair[i]` has `field[11]` pointing at the OT index
    ///    of the name referenced by `pair[i+1]`. (The "shift-by-one link"
    ///    -- each record's `field[11]` names its file-order successor's
    ///    slot, not its own.)
    /// 5. `Time` is bound to OT[0] implicitly (Time is the only name with
    ///    `ot_index == 0`; the first record's `field[11]` carries the OT
    ///    pointer for the name after Time).
    ///
    /// Names that cannot own an OT entry (dot-prefix view markers, unit
    /// annotations `-...`, Vensim builtins, `:...` metadata, `?` placeholders)
    /// are skipped. Records with `field[11] == 0` are treated as a sentinel
    /// meaning "the next name has no OT entry" -- the only exception is the
    /// name `"Time"`, which always lives at OT[0]. The sentinel is not
    /// perfect: on WRLD3 SCEN01 a handful of real model variables also
    /// appear as `field[11] == 0` successors (`unit agricultural input` and
    /// several `#SMOOTH3(...)#` signature names). Those mappings are lost
    /// by this path; a fully deterministic decoder would need an additional
    /// signal beyond f[11]==0 to separate aliases from real variables.
    ///
    /// For records whose `field[6]` indicates an arrayed shape (i.e. `f[6]
    /// != 5` and `f[6] != 0`), the OT block start from the shift-by-one
    /// link is expanded to `N` consecutive OT slots, where `N` is the
    /// `flat_size` of the matching section-3 shape entry. Each element
    /// receives the label `name[i]` (0-indexed), matching the convention
    /// used in [`VdfFile::to_results_via_records`].
    ///
    /// On small single-view fixtures (water, pop, bact, ...) and on the
    /// multi-view WRLD3 files this yields the canonical mapping Vensim
    /// itself recovers when opening the VDF without its MDL. On large
    /// compilation-order files (`Ref.vdf`) and edited/re-saved files
    /// (`risk2.vdf`) the mapping is approximate because the file-order
    /// pair stream itself drifts on unmatched dot names / orphan headers
    /// (see diagnostics returned by
    /// [`Self::record_view_groups_with_diagnostics`]).
    pub fn to_results_via_file_order_records(&self) -> StdResult<Results, Box<dyn Error>> {
        let vdf_data = self.extract_data()?;

        let (pairs, _diag) = self.build_file_order_pairs();
        let section3_directory = self.parse_section3_directory();

        let mut ordered: Vec<(Ident<Canonical>, usize)> =
            vec![(Ident::<Canonical>::from_str_unchecked("time"), 0)];
        let mut claimed_ot: HashSet<usize> = HashSet::new();
        claimed_ot.insert(0);

        // Shift-by-one: rec[pairs[i]].field[11] owns the OT for pairs[i+1].
        // The *shape* of name[pairs[i+1]] is determined by the record
        // paired with that name, i.e. rec[pairs[i+1].0].
        for window in pairs.windows(2) {
            let (pred_rec_idx, _) = window[0];
            let (rec_idx, next_name_idx) = window[1];
            let pred_rec = &self.records[pred_rec_idx];
            let ot_start = pred_rec.fields[11] as usize;
            if ot_start >= self.offset_table_count {
                continue;
            }
            let name = &self.names[next_name_idx];
            if name.is_empty() || name_has_no_ot(name) {
                continue;
            }
            // Sentinel: field[11] == 0 usually means "no OT for the next
            // name". Time is the single variable that does live at OT[0],
            // and it is paired above explicitly.
            if ot_start == 0 && !name_matches_time(name) {
                continue;
            }

            // Determine the OT span. Scalar records (f[6] == 5) consume a
            // single slot. Arrayed records (f[6] != 5 and f[6] != 0)
            // consume a contiguous block whose length is the section-3
            // shape's flat_size. This mirrors the logic in
            // `to_results_via_records`.
            let self_rec = &self.records[rec_idx];
            let span = ot_span_for_record(self_rec, &section3_directory, self.offset_table_count);
            let span = match span {
                OtSpan::Scalar => 1usize,
                OtSpan::Arrayed(n) => n,
                OtSpan::UnknownShape => continue,
                OtSpan::NoShape => 1usize,
            };
            if ot_start + span > self.offset_table_count {
                continue;
            }

            for elem in 0..span {
                let ot = ot_start + elem;
                if !claimed_ot.insert(ot) {
                    continue;
                }
                let display = if span > 1 {
                    format!("{name}[{elem}]")
                } else {
                    name.clone()
                };
                let key = if display.starts_with('#') {
                    Ident::<Canonical>::from_str_unchecked(&display)
                } else {
                    Ident::<Canonical>::new(&display)
                };
                ordered.push((key, ot));
            }
        }

        Ok(vdf_data.build_results(&ordered))
    }

    /// Build the file-order record-to-name pairing used by
    /// [`VdfFile::to_results_via_file_order_records`], alongside a
    /// diagnostics record that surfaces unmatched headers / dot names.
    ///
    /// Returns `(pairs, diag)` where `pairs` is `Vec<(record_idx,
    /// name_idx)>` with variable records paired with non-dot names and
    /// view-header records paired with dot-prefix names, traversing both
    /// streams in file order. `diag` lists any names or records that
    /// could not be paired so callers can detect a fixture where the 1:1
    /// pairing assumption breaks.
    pub(crate) fn build_file_order_pairs(&self) -> (Vec<(usize, usize)>, FileOrderPairDiagnostics) {
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        let mut diag = FileOrderPairDiagnostics::default();
        let mut rec_idx = 0usize;
        let mut name_idx = 0usize;
        while rec_idx < self.records.len() && name_idx < self.names.len() {
            let is_header = self.records[rec_idx].is_view_header();
            let mut advanced = false;
            while name_idx < self.names.len() {
                let is_dot = self.names[name_idx].starts_with('.');
                if is_header == is_dot {
                    advanced = true;
                    break;
                }
                if is_dot {
                    diag.skipped_dot_names.push(name_idx);
                } else {
                    diag.skipped_non_dot_names.push(name_idx);
                }
                name_idx += 1;
            }
            if !advanced {
                break;
            }
            pairs.push((rec_idx, name_idx));
            name_idx += 1;
            rec_idx += 1;
        }
        for leftover in rec_idx..self.records.len() {
            let rec = &self.records[leftover];
            if rec.is_view_header() {
                diag.unpaired_headers.push(leftover);
            } else {
                diag.unpaired_records.push(leftover);
            }
        }
        for leftover in name_idx..self.names.len() {
            let name = &self.names[leftover];
            if name.starts_with('.') {
                diag.unpaired_dot_names.push(leftover);
            } else {
                diag.unpaired_non_dot_names.push(leftover);
            }
        }
        (pairs, diag)
    }
}

/// Internal result of pairing header records with dot-prefix names by
/// file order. Retained for both the public
/// [`ViewBlockDiagnostics`] API and internal uses.
struct HeaderDotPairing {
    header_rec_indices: Vec<usize>,
    dot_name_indices: Vec<usize>,
    unmatched_headers: Vec<usize>,
    unmatched_dots: Vec<usize>,
}

/// Accounting returned alongside a file-order pairing so callers can
/// quantify how much of the file was successfully paired and which
/// records / names fell outside the 1:1 structure.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileOrderPairDiagnostics {
    /// Dot-prefix names that were skipped during pairing because the
    /// record cursor was on a non-header record when they appeared in
    /// name-table order.
    pub skipped_dot_names: Vec<usize>,
    /// Non-dot names that were skipped during pairing because the record
    /// cursor was on a header record.
    pub skipped_non_dot_names: Vec<usize>,
    /// Header records left over after the name stream was exhausted.
    /// Observed on `risk2.vdf` where the `.Control` name was trimmed but
    /// its header record was retained.
    pub unpaired_headers: Vec<usize>,
    /// Non-header records left over after the name stream was exhausted.
    /// Observed on re-saved files (`risk2.vdf`) and on `Ref.vdf` where
    /// tail records exist past every named variable.
    pub unpaired_records: Vec<usize>,
    /// Dot-prefix names left over after the record stream was exhausted.
    pub unpaired_dot_names: Vec<usize>,
    /// Non-dot names left over after the record stream was exhausted.
    pub unpaired_non_dot_names: Vec<usize>,
}

/// The OT-span decision for a single variable record.
enum OtSpan {
    /// Scalar record (`f[6] == 5`): 1 OT slot.
    Scalar,
    /// Arrayed record with a resolved shape: `n` consecutive OT slots.
    Arrayed(usize),
    /// Arrayed record whose shape could not be resolved (e.g., the
    /// section-3 directory lacks a matching entry and the file has
    /// multiple shape templates). The caller should skip.
    UnknownShape,
    /// Record with `f[6] == 0` (padding / no shape). Callers treat this
    /// as 1 slot to preserve pre-existing behavior on auxiliary records.
    NoShape,
}

fn ot_span_for_record(
    rec: &super::VdfRecord,
    section3: &Option<super::VdfSection3Directory>,
    _ot_count: usize,
) -> OtSpan {
    let shape_code = rec.fields[6];
    if shape_code == 5 {
        return OtSpan::Scalar;
    }
    if shape_code == 0 {
        return OtSpan::NoShape;
    }
    // Arrayed: look up section-3 directory entry with matching index_word.
    if let Some(dir) = section3.as_ref() {
        for entry in &dir.entries {
            if entry.index_word() == shape_code && entry.flat_size() > 0 {
                return OtSpan::Arrayed(entry.flat_size());
            }
        }
        // shape_code == 32 is the generic arrayed marker used in
        // single-shape files. Resolve it to the sole non-zero flat size
        // when exactly one is present.
        if shape_code == 32 {
            let sizes: HashSet<usize> = dir
                .entries
                .iter()
                .map(|e| e.flat_size())
                .filter(|&s| s > 0)
                .collect();
            if sizes.len() == 1
                && let Some(&n) = sizes.iter().next()
            {
                return OtSpan::Arrayed(n);
            }
        }
    }
    OtSpan::UnknownShape
}

/// Names that cannot own an OT entry. These are filtered from the
/// file-order shift-by-one pairing in
/// [`VdfFile::to_results_via_file_order_records`].
///
/// - Names starting with `'-'` are unit annotations (`-Ghectares`).
/// - Names starting with `':'` are metadata tags.
/// - Names starting with `'?'` are single-char placeholders.
/// - Names in a fixed builtin list (`SUM`, `SMOOTH`, `IN`, `PI`, etc.) are
///   Vensim operators that appear in the name table for lookup/display but
///   never receive an OT slot.
///
/// Dot-prefix names (`.Agriculture`, ...) are also never OT owners, but
/// the file-order pairing routes them to view-header records, so they are
/// skipped by construction and do not need this filter.
fn name_has_no_ot(name: &str) -> bool {
    if name.is_empty() {
        return true;
    }
    if let Some(first) = name.chars().next()
        && matches!(first, '-' | ':' | '?')
    {
        return true;
    }
    matches!(
        name,
        "SUM"
            | "PROD"
            | "VMIN"
            | "VMAX"
            | "LOG"
            | "MIN"
            | "MAX"
            | "ABS"
            | "EXP"
            | "LN"
            | "SMOOTH"
            | "DELAY1"
            | "DELAY3"
            | "SMOOTH3"
            | "SMOOTHI"
            | "TREND"
            | "IN"
            | "INI"
            | "OUTPUT"
            | "PI"
            | "SIN"
            | "COS"
            | "TAN"
            | "SQRT"
            | "STEP"
            | "INTEGER"
            | "RAMP"
            | "PULSE"
            | "MODULO"
    )
}

/// The "Time" name that owns OT[0] in every VDF. Compared case-insensitively
/// because small fixtures differ in casing.
fn name_matches_time(name: &str) -> bool {
    name.eq_ignore_ascii_case("Time")
}

#[cfg(test)]
mod tests {
    use super::super::VdfFile;

    fn vdf_file(path: &str) -> VdfFile {
        let data =
            std::fs::read(path).unwrap_or_else(|e| panic!("failed to read VDF file {path}: {e}"));
        VdfFile::parse(data).unwrap_or_else(|e| panic!("failed to parse VDF file {path}: {e}"))
    }

    /// Fixtures where the f[1]==138 -> dot-prefix alignment holds 1:1.
    /// Every small single-view fixture plus WRLD3 SCEN01/experiment pair
    /// the header count exactly with the dot-prefix count.
    #[test]
    fn test_record_view_groups_one_to_one_fixtures() {
        for vdf_path in [
            "../../test/bobby/vdf/water/water.vdf",
            "../../test/bobby/vdf/pop/pop.vdf",
            "../../test/bobby/vdf/bact/euler.vdf",
            "../../test/bobby/vdf/consts/b_is_3.vdf",
            "../../test/bobby/vdf/level_vs_aux/x_is_aux.vdf",
            "../../test/bobby/vdf/level_vs_aux/x_is_stock.vdf",
            "../../test/bobby/vdf/lookups/lookup_ex.vdf",
            "../../test/bobby/vdf/subscripts/subscripts.vdf",
            "../../test/bobby/vdf/model_editing/run_5.vdf",
            "../../test/bobby/vdf/econ/base.vdf",
            "../../test/bobby/vdf/econ/risk.vdf",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
            "../../test/metasd/WRLD3-03/experiment.vdf",
        ] {
            let vdf = vdf_file(vdf_path);
            let (groups, diag) = vdf.record_view_groups_with_diagnostics();
            assert_eq!(
                diag.view_header_count, diag.dot_prefix_name_count,
                "{vdf_path}: view-header record count {} must match \
                 dot-prefix name count {}",
                diag.view_header_count, diag.dot_prefix_name_count
            );
            assert!(
                diag.unmatched_header_rec_indices.is_empty(),
                "{vdf_path}: no headers should be unmatched; got {:?}",
                diag.unmatched_header_rec_indices
            );
            assert!(
                diag.unmatched_dot_name_indices.is_empty(),
                "{vdf_path}: no dot-prefix names should be unmatched; got {:?}",
                diag.unmatched_dot_name_indices
            );
            assert_eq!(
                groups.len(),
                diag.view_header_count + 1,
                "{vdf_path}: record_view_groups must produce header_count+1 groups"
            );

            let sum_of_groups: usize = groups.iter().map(|g| g.len()).sum();
            let expected_non_headers = vdf.records.len() - diag.view_header_count;
            assert_eq!(
                sum_of_groups, expected_non_headers,
                "{vdf_path}: groups must cover every non-header record"
            );
        }
    }

    /// Fixtures where the 1:1 alignment does NOT hold -- these are
    /// edited/re-saved files or large compilation-order files. The test
    /// pins the observed divergence so a future change to either
    /// `is_view_header` or the fixture set surfaces as a test failure.
    ///
    /// - `risk2.vdf` has 2 header records vs 1 dot-prefix name
    ///   (the user dropped `.Control` but the record survived).
    /// - `Ref.vdf` (C-LEARN) has 17 header records vs 69 dot-prefix
    ///   names because many dot names describe sub-groups within a
    ///   parent view and do not carry their own header records.
    #[test]
    fn test_record_view_groups_divergent_fixtures() {
        let vdf = vdf_file("../../test/bobby/vdf/econ/risk2.vdf");
        let (_groups, diag) = vdf.record_view_groups_with_diagnostics();
        assert_eq!(diag.view_header_count, 2, "risk2.vdf pins 2 header records");
        assert_eq!(
            diag.dot_prefix_name_count, 1,
            "risk2.vdf pins 1 dot-prefix name"
        );
        assert_eq!(
            diag.unmatched_header_rec_indices.len(),
            1,
            "risk2.vdf has exactly 1 orphan header record"
        );
        assert!(
            diag.unmatched_dot_name_indices.is_empty(),
            "risk2.vdf has no orphan dot names"
        );

        let vdf = vdf_file("../../test/xmutil_test_models/Ref.vdf");
        let (_groups, diag) = vdf.record_view_groups_with_diagnostics();
        assert_eq!(diag.view_header_count, 17, "Ref.vdf pins 17 header records");
        assert_eq!(
            diag.dot_prefix_name_count, 69,
            "Ref.vdf pins 69 dot-prefix names"
        );
        assert!(
            diag.unmatched_header_rec_indices.is_empty(),
            "Ref.vdf: 17 headers all have matching dot names"
        );
        assert_eq!(
            diag.unmatched_dot_name_indices.len(),
            52,
            "Ref.vdf: 52 dot-prefix names lack dedicated header records \
             (mostly sub-group names like '.Agriculture.Loop1')"
        );
    }

    /// Within each non-last view block, the number of records must match
    /// the number of names between consecutive dot-prefix markers. This
    /// invariant holds on multi-view fixtures (WRLD3) where every
    /// dot-prefix name pairs with a view-header record and the records
    /// between consecutive headers correspond to the names between
    /// consecutive dot markers.
    ///
    /// On small single-view fixtures (e.g. `b_is_3.vdf`, subscripts)
    /// the invariant does NOT hold because:
    /// - The file-order records include dimension-element records that
    ///   don't participate in the single user view.
    /// - Sometimes two f[1]==138 headers sit back-to-back with no records
    ///   between them, creating an empty middle block.
    ///
    /// Those fixtures are excluded here; the one-to-one count assertion
    /// in `test_record_view_groups_one_to_one_fixtures` still pins their
    /// top-level structure.
    #[test]
    fn test_record_view_groups_match_name_block_sizes() {
        for vdf_path in [
            "../../test/bobby/vdf/water/water.vdf",
            "../../test/bobby/vdf/pop/pop.vdf",
            "../../test/bobby/vdf/bact/euler.vdf",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
            "../../test/metasd/WRLD3-03/experiment.vdf",
        ] {
            let vdf = vdf_file(vdf_path);
            let (groups, diag) = vdf.record_view_groups_with_diagnostics();

            // Only meaningful on 1:1 fixtures.
            if diag.view_header_count != diag.dot_prefix_name_count {
                continue;
            }

            let dot_positions: Vec<usize> = vdf
                .names
                .iter()
                .enumerate()
                .filter_map(|(i, n)| if n.starts_with('.') { Some(i) } else { None })
                .collect();

            let mut name_block_sizes: Vec<usize> = Vec::with_capacity(dot_positions.len() + 1);
            name_block_sizes.push(dot_positions.first().copied().unwrap_or(vdf.names.len()));
            for window in dot_positions.windows(2) {
                name_block_sizes.push(window[1] - window[0] - 1);
            }
            if let Some(&last) = dot_positions.last() {
                name_block_sizes.push(vdf.names.len() - last - 1);
            }

            assert_eq!(
                groups.len(),
                name_block_sizes.len(),
                "{vdf_path}: group count {} must match name block count {}",
                groups.len(),
                name_block_sizes.len()
            );

            // All non-last blocks must match exactly. The last block is
            // excluded because `#` signatures and stdlib tail records
            // live past the slotted prefix on WRLD3-style files; on
            // small fixtures the invariant still holds but we skip
            // uniformly to keep the test simple.
            for (i, (group, &expected)) in groups
                .iter()
                .zip(name_block_sizes.iter())
                .enumerate()
                .take(groups.len().saturating_sub(1))
            {
                assert_eq!(
                    group.len(),
                    expected,
                    "{vdf_path}: block {i} record count {} must match name block count {}",
                    group.len(),
                    expected
                );
            }
        }
    }

    /// On every valid fixture (including the divergent `risk2.vdf` and
    /// `Ref.vdf`), `to_results_via_file_order_records` must succeed
    /// without panicking. This pins graceful-handling behavior.
    #[test]
    fn test_to_results_via_file_order_records_does_not_panic() {
        for vdf_path in [
            "../../test/bobby/vdf/water/water.vdf",
            "../../test/bobby/vdf/pop/pop.vdf",
            "../../test/bobby/vdf/econ/base.vdf",
            "../../test/bobby/vdf/econ/risk.vdf",
            "../../test/bobby/vdf/econ/risk2.vdf",
            "../../test/bobby/vdf/subscripts/subscripts.vdf",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
            "../../test/metasd/WRLD3-03/experiment.vdf",
            "../../test/xmutil_test_models/Ref.vdf",
        ] {
            let vdf = vdf_file(vdf_path);
            let _ = vdf
                .to_results_via_file_order_records()
                .unwrap_or_else(|e| panic!("{vdf_path}: file-order mapping should succeed: {e}"));
        }
    }

    /// On the `subscripts` fixture, arrayed records now expand to
    /// multiple OT slots using the section-3 shape (`net_flow[0]`,
    /// `net_flow[1]`, etc.) instead of silently collapsing to a single
    /// slot. However, subscripts.vdf is NOT a clean fixture for the
    /// file-order pairing: the name table includes dimension-element
    /// names `a`, `b`, `c` that occupy pairing slots with non-variable
    /// records, shifting the 1:1 record-to-name correspondence.
    /// `to_results_via_records` (f[2]-sort based) is the authoritative
    /// path for this fixture; the file-order path surfaces a partial
    /// mapping only.
    ///
    /// This test pins the observed behavior after the arrayed-expansion
    /// fix: the column count must be strictly greater than the pre-fix
    /// baseline (8 columns, one per scalar name) so future silent
    /// regressions surface immediately.
    #[test]
    fn test_to_results_via_file_order_records_expands_arrayed_on_subscripts() {
        let vdf = vdf_file("../../test/bobby/vdf/subscripts/subscripts.vdf");
        let results = vdf
            .to_results_via_file_order_records()
            .expect("subscripts mapping should succeed");
        // Pre-fix baseline was 8 columns (all arrayed variables collapsed
        // to a single slot). With arrayed expansion we see 9+ columns.
        // We do NOT reach the full 15 because the dim-element names shift
        // the pairing; see `to_results_via_records` for the deterministic
        // f[2]-sort alternative that recovers all 15.
        let col_count = results.offsets.len();
        assert!(
            col_count >= 9,
            "subscripts: arrayed records must be expanded; got {col_count} \
             columns, expected >= 9 (pre-fix baseline was 8 scalar-only columns)"
        );
        // Confirm at least one arrayed element label appears (proving
        // the expansion happened rather than simply filling more
        // scalar names).
        let has_arrayed_label = results.offsets.keys().any(|k| k.as_str().contains('['));
        assert!(
            has_arrayed_label,
            "subscripts: expected at least one `name[i]` element label; got {:?}",
            results
                .offsets
                .keys()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
        );
    }

    /// On WRLD3 SCEN01 and experiment, compare the file-order mapping
    /// against `build_section6_guided_ot_map` via per-time-point
    /// time-series equality. This replaces the earlier "constants
    /// final-value match" test which had a statistical blind spot: many
    /// WRLD3 constants share values (e.g. 1.0 appears on 36 OT slots),
    /// so a randomly-mismatched mapping could still score high on
    /// constants but fail on varying data.
    ///
    /// Per-time-point equality eliminates that blind spot: two distinct
    /// OT slots are never confused because their full 401-step series
    /// differ byte-for-byte.
    ///
    /// The two paths disagree significantly on WRLD3 -- the file-order
    /// path and the model-guided path use different mapping heuristics
    /// with different failure modes. We pin the observed agreement
    /// ranges so regressions in either direction surface; we do not
    /// claim one is uniformly more correct than the other.
    #[test]
    fn test_to_results_via_file_order_records_agrees_with_guided_on_wrld3() {
        use crate::compat::open_vensim;

        for (label, vdf_path, mdl_path, min_agreed, min_compared) in [
            (
                "WRLD3 SCEN01",
                "../../test/metasd/WRLD3-03/SCEN01.VDF",
                "../../test/metasd/WRLD3-03/wrld3-03.mdl",
                // Observed: 42/266 names agree on exact time series.
                // Floor of 30 pins the invariant that SOME real
                // agreement exists (regression guard); higher is
                // fine and would indicate the paths have converged
                // further.
                30usize,
                200usize,
            ),
            (
                "WRLD3 experiment",
                "../../test/metasd/WRLD3-03/experiment.vdf",
                "../../test/metasd/WRLD3-03/wrld3-03.mdl",
                30usize,
                200usize,
            ),
        ] {
            let vdf = vdf_file(vdf_path);
            let contents = std::fs::read_to_string(mdl_path)
                .unwrap_or_else(|e| panic!("{label}: read {mdl_path}: {e}"));
            let datamodel_project =
                open_vensim(&contents).unwrap_or_else(|e| panic!("{label}: parse mdl: {e:?}"));
            let model = datamodel_project
                .models
                .first()
                .unwrap_or_else(|| panic!("{label}: empty datamodel"));

            let via = vdf
                .to_results_via_file_order_records()
                .unwrap_or_else(|e| panic!("{label}: file-order mapping should succeed: {e}"));
            let guided = vdf
                .build_section6_guided_ot_map(model)
                .unwrap_or_else(|e| panic!("{label}: guided map failed: {e}"));
            let vdf_data = vdf.extract_data().expect("extract data");

            let mut compared = 0usize;
            let mut agreed = 0usize;
            for (name, &col) in &via.offsets {
                let Some(&guided_ot) = guided.get(name) else {
                    continue;
                };
                let Some(guided_series) = vdf_data.entries.get(guided_ot) else {
                    continue;
                };
                compared += 1;
                let step_size = via.offsets.len();
                let all_match =
                    guided_series
                        .iter()
                        .take(via.step_count)
                        .enumerate()
                        .all(|(step, &g)| {
                            let v = via.data[step * step_size + col];
                            (v - g).abs() <= 1e-6
                        });
                if all_match {
                    agreed += 1;
                }
            }
            assert!(
                compared >= min_compared,
                "{label}: expected >={min_compared} overlapping names, got {compared}"
            );
            assert!(
                agreed >= min_agreed,
                "{label}: time-series agreement {agreed}/{compared} below floor \
                 {min_agreed} -- did the mapping regress?"
            );
        }
    }

    /// On WRLD3, quantify the `f[11]==0` "no OT" sentinel: among the
    /// names it filters out, which have real semantic correspondence to
    /// a recoverable OT vs. are true no-OT entries (aliases, units,
    /// metadata).
    ///
    /// This test pins the observed behavior so a future change to the
    /// sentinel rule surfaces as a measurable difference.
    #[test]
    fn test_field11_zero_sentinel_loss_on_wrld3_is_pinned() {
        for (label, vdf_path, expected_successor_count) in [
            ("SCEN01", "../../test/metasd/WRLD3-03/SCEN01.VDF", 59usize),
            (
                "experiment",
                "../../test/metasd/WRLD3-03/experiment.vdf",
                36usize,
            ),
        ] {
            let vdf = vdf_file(vdf_path);
            let (pairs, _diag) = vdf.build_file_order_pairs();
            let mut zero_successor_count = 0usize;
            for window in pairs.windows(2) {
                let (pred_rec_idx, _) = window[0];
                if vdf.records[pred_rec_idx].fields[11] == 0 {
                    zero_successor_count += 1;
                }
            }
            assert_eq!(
                zero_successor_count, expected_successor_count,
                "{label}: field[11]==0 successor count drift"
            );
        }
    }
}
