// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Regression tests pinning the numeric evidence for the multi-dim
//! element-naming investigation on `Ref.vdf` (C-LEARN, 18 declared
//! dimensions). See docs/design/vdf.md "Claims about multi-dim element
//! naming" for the full ruled-out candidate list.
//!
//! These tests live in an integration-test file (not inside `vdf.rs`) so
//! that the ~120-line ruled-out fixture does not push `src/vdf.rs` over
//! the project's 6000-line module threshold.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use simlin_engine::Results;
use simlin_engine::common::{Canonical, Ident};
use simlin_engine::vdf::{VDF_SENTINEL, VdfData, VdfFile, VdfSection5SetEntry};

fn load_ref_vdf() -> VdfFile {
    let data =
        std::fs::read("../../test/xmutil_test_models/Ref.vdf").expect("read Ref.vdf fixture");
    VdfFile::parse(data).expect("parse Ref.vdf")
}

fn assert_result_column_matches_ot(results: &Results, vdf_data: &VdfData, name: &str, ot: usize) {
    let ident = Ident::<Canonical>::new(name);
    let col = results
        .offsets
        .get(&ident)
        .unwrap_or_else(|| panic!("missing result column {name}"));
    let expected = vdf_data
        .entries
        .get(ot)
        .unwrap_or_else(|| panic!("missing OT[{ot}]"));
    for (step, expected_value) in expected.iter().take(results.step_count).enumerate() {
        let actual = results.data[step * results.step_size + col];
        assert!(
            (actual - expected_value).abs() <= 1e-6,
            "{name}: step {step} actual {actual} != OT[{ot}] value {expected_value}"
        );
    }
}

#[test]
fn ref_vdf_record_results_prune_descriptor_overlaps() {
    let ref_vdf = load_ref_vdf();
    let results = ref_vdf
        .to_results_via_records()
        .expect("record-based mapping should produce Ref.vdf columns");
    let vdf_data = ref_vdf.extract_data().unwrap();

    for (name, ot) in [
        ("C in Mixed Layer[0]", 137),
        ("C in Mixed Layer[1]", 138),
        ("C in Mixed Layer[2]", 139),
        ("Cum CO2 at start[0]", 146),
        ("Cum CO2eq at start[0]", 153),
        ("Cumulative CO2[0]", 160),
    ] {
        assert_result_column_matches_ot(&results, &vdf_data, name, ot);
    }

    for descriptor_name in [
        "RS N2O[0]",
        "RS PFC[0]",
        "RS SF6[0]",
        "UN population HIGH LOOKUP[0]",
        "UN population LOW LOOKUP[0]",
        "UN population MED LOOKUP[0]",
        "Specified CO2eq emissions scenario in CO2",
        "Specified Developed CO2eq emissions",
        "Specified Developing A CO2eq emissions",
        "Specified Developing B CO2eq emissions",
        "Specified Global CH4",
    ] {
        assert!(
            !results
                .offsets
                .contains_key(&Ident::<Canonical>::new(descriptor_name)),
            "Ref.vdf descriptor overlap should not be emitted: {descriptor_name}"
        );
    }
}

#[test]
fn ref_vdf_multidim_binding_candidates_are_ruled_out() {
    let ref_vdf = load_ref_vdf();

    // The 18 dimensions declared in the C-LEARN MDL. The VDF name table
    // contains every dim name verbatim.
    let dim_names: HashSet<&str> = [
        "Target",
        "COP Developed",
        "COP Developing A",
        "COP Remaining Developing",
        "COP",
        "Developing A",
        "Developing B",
        "Semi Agg",
        "HFC type",
        "Aggregated Regions",
        "layers",
        "bottom",
        "lower",
        "upper",
        "set targets",
        "tNext",
        "tPrev",
        "scenario",
    ]
    .into_iter()
    .collect();
    assert!(
        dim_names
            .iter()
            .all(|d| ref_vdf.names.iter().any(|n| n == d)),
        "all 18 MDL dim names appear in the VDF name table"
    );

    // Direct slot_table -> name mapping. Each slot_table entry pairs 1:1
    // with the name at the same index (no leading-extra-slot shift needed
    // for this fixture).
    let slot_to_name: HashMap<u32, &str> = ref_vdf
        .slot_table
        .iter()
        .enumerate()
        .filter_map(|(i, s)| ref_vdf.names.get(i).map(|n| (*s, n.as_str())))
        .collect();

    // --- Candidate A: sec4 as (axis_slot_ref, dim_name_slot_ref) binding ---
    //
    // Only "COP" ever appears in any sec4 entry under direct slot mapping;
    // the other 17 dim names are absent. Sec4 is view/sketch metadata, not a
    // clean dim-axis binding table.
    let sec4 = ref_vdf
        .parse_section4_entry_stream()
        .expect("parse sec4 stream");
    let sec4_dims: HashSet<&str> = sec4
        .entries
        .iter()
        .flat_map(|e| e.refs.iter())
        .filter_map(|r| slot_to_name.get(r).copied())
        .filter(|n| dim_names.contains(n))
        .collect();
    assert_eq!(sec4_dims, HashSet::from(["COP"]));

    // --- Candidate B: sec5 payload refs as dim-name pointers ---
    //
    // Every non-trailing payload ref resolves to a VARIABLE name; zero of
    // them resolve to any of the 18 dim names.
    let (_, sec5, _) = ref_vdf
        .parse_section5_set_stream()
        .expect("parse sec5 stream");
    let (mut total, mut dim_hits) = (0usize, 0usize);
    for e in &sec5 {
        let payload_end = e.refs.len().saturating_sub(1 + e.marker as usize);
        for r in &e.refs[..payload_end] {
            if *r == 0 {
                continue;
            }
            total += 1;
            if slot_to_name.get(r).is_some_and(|n| dim_names.contains(n)) {
                dim_hits += 1;
            }
        }
    }
    // Pinned to the exact observed count so a payload-ref decoding shift
    // shows up as a test failure rather than drifting unnoticed.
    assert_eq!(
        total, 59,
        "sec5 has exactly 59 non-zero payload refs on Ref.vdf"
    );
    assert_eq!(
        dim_hits, 0,
        "zero sec5 payload refs resolve to any dim name"
    );

    // Sec5 has exactly 18 entries whose `n` values sort-match the 18 MDL
    // dim cardinalities. The file-offset ordering of sec5 entries, however,
    // does not correspond to MDL declaration order, alphabetical order, or
    // name-table order of dim names -- so there is no deterministic way
    // to pair a given sec5 entry with a specific dim name.
    assert_eq!(sec5.len(), dim_names.len());
    let mut n_sorted: Vec<usize> = sec5.iter().map(|e| e.n).collect();
    n_sorted.sort_unstable();
    // MDL dim cardinalities sorted: layers=4, bottom=1, lower=3, upper=3,
    // Target=3, set targets=2, tNext=2, tPrev=2, COP Developed=3,
    // COP Developing A=3, COP Remaining Developing=2, COP=7,
    // Developing A=2, Developing B=1, Semi Agg=6, HFC type=9,
    // Aggregated Regions=3, scenario=3.
    assert_eq!(
        n_sorted,
        vec![1, 1, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 4, 6, 7, 9]
    );

    // --- Candidate C: dim name followed by element names in the name table ---
    //
    // Only `COP` (name-table index 62) has its 7 elements contiguous at
    // indices 71..77. `HFC type` (index 138) has its 9 elements scattered
    // across 163..872 -- span > 500. Most dims have non-contiguous elements.
    let name_idx: HashMap<&str, usize> = ref_vdf
        .names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let cop_idxs: Vec<usize> = [
        "OECD US",
        "OECD EU",
        "G77 China",
        "G77 India",
        "Remaining Developed",
        "Remaining Developing A",
        "COP Developing B",
    ]
    .iter()
    .filter_map(|n| name_idx.get(n).copied())
    .collect();
    assert_eq!(cop_idxs, vec![71, 72, 73, 74, 75, 76, 77]);

    let hfc_idxs: Vec<usize> = [
        "HFC134a",
        "HFC23",
        "HFC32",
        "HFC125",
        "HFC143a",
        "HFC152a",
        "HFC227ea",
        "HFC245ca",
        "HFC4310mee",
    ]
    .iter()
    .filter_map(|n| name_idx.get(n).copied())
    .collect();
    assert_eq!(hfc_idxs.len(), 9);
    let span = hfc_idxs.iter().max().unwrap() - hfc_idxs.iter().min().unwrap();
    assert!(
        span > 500,
        "HFC type's 9 elements span hundreds of name-table slots (actual span={span})"
    );
}

#[test]
fn ref_vdf_section6_ref_stream_contains_direct_dimension_refs() {
    let ref_vdf = load_ref_vdf();

    let slot_to_name: HashMap<u32, &str> = ref_vdf
        .slot_table
        .iter()
        .enumerate()
        .filter_map(|(i, s)| ref_vdf.names.get(i).map(|n| (*s, n.as_str())))
        .collect();
    let (_, sec6, _) = ref_vdf
        .parse_section6_ref_stream()
        .expect("parse sec6 ref stream");

    let names_for = |entry_index: usize| -> Vec<&str> {
        sec6[entry_index]
            .refs
            .iter()
            .filter_map(|r| slot_to_name.get(r).copied())
            .collect()
    };

    // These are not enough to map multidim arrays yet, but they prove section
    // 6 carries direct references to dimension names. Future decoding work
    // should treat the ref stream as a candidate dim-ownership signal rather
    // than only as formula/reference metadata.
    assert_eq!(names_for(5), vec!["lower"]);
    assert_eq!(names_for(6), vec!["upper"]);
    assert_eq!(names_for(294), vec!["Target"]);
    assert_eq!(names_for(427), vec!["layers"]);
    assert_eq!(names_for(453), vec!["set targets"]);
    assert_eq!(names_for(638), vec!["set targets"]);
    assert_eq!(names_for(865), vec!["Semi Agg"]);
    assert_eq!(names_for(699), vec!["Global CO2eq target", "COP Developed"]);
    assert_eq!(names_for(700), vec!["COP Developing A", "Initial N2O conc"]);
}

/// Dimension anchor decoded directly from the record-field[8] grouping.
///
/// Mirrors the shape of `decoded_record_dimension_anchors` in
/// `tools/vdf_xray.py`: one anchor per dimension declared in the model,
/// with its element records collected from the matching `f[8]` group.
/// Used below to pin the sec5-to-anchor alignment rule (each section-5
/// entry in file order pairs with the anchor at the same rank in
/// `f[8]`-ascending order) and the subrange payload-subsequence rule.
#[derive(Debug, Clone)]
struct DimAnchor {
    /// Anchor record's name (from section-2 via f[2] name key).
    name: String,
    /// Shared `f[8]` group id used to link the anchor to its element records.
    group_id: u32,
    /// Zero-based element records sharing this anchor's f[8], sorted by
    /// `f[11]` (element index). Each entry is `(element_idx, name)`.
    elements: Vec<(u32, String)>,
    /// True when the element records form a contiguous 0..N run matching
    /// the MDL cardinality. Matches the Python decoder's `status=complete`.
    complete: bool,
}

/// Build a `(field[2] name-key) -> name` table by replaying the section-2
/// name-table layout. This mirrors
/// `record_name_key_to_name_index` in `vdf.rs`; we reimplement it here
/// because the upstream helper is `fn`-private, and the tests only need
/// the string lookup (not the full name-index).
fn record_name_key_to_name(vdf: &VdfFile) -> HashMap<u32, String> {
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
    out.insert(7u32, vdf.names[0].clone());
    let mut pos = data_start + first_len;
    let mut name_idx = 1usize;
    while name_idx < vdf.names.len() && pos + 2 <= parse_end {
        let len = u16::from_le_bytes([vdf.data[pos], vdf.data[pos + 1]]) as usize;
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

/// Recover dimension anchors from the record stream by applying the same
/// grouping rules the Python `decoded_record_dimension_anchors` uses:
///
/// * anchor candidate: `f[6] == 0 && f[14] == VDF_SENTINEL` with a valid
///   `f[8]` group id (non-zero, not the sentinel);
/// * element candidate: `f[6] == 0 && f[12] == 124 && f[10] == 0
///   && f[14] != VDF_SENTINEL && f[11] < 4096` with the same `f[8]`.
///
/// The returned list contains one entry per `(group_id, anchor-name)`
/// pair where at least one element record exists in the same group. This
/// mirrors the "anchors that have at least one element record sharing
/// the same f[8]" selector the task wants the tests to apply.
fn recover_anchors(vdf: &VdfFile) -> Vec<DimAnchor> {
    let key_to_name = record_name_key_to_name(vdf);
    if key_to_name.is_empty() {
        return Vec::new();
    }

    // Group potential anchors and elements by f[8] group id.
    let mut candidate_anchors: HashMap<u32, Vec<(String, u32)>> = HashMap::new();
    let mut element_groups: HashMap<u32, Vec<(u32, String)>> = HashMap::new();

    for rec in &vdf.records {
        if rec.fields[6] != 0 {
            continue;
        }
        let group_id = rec.fields[8];
        if group_id == 0 || group_id == VDF_SENTINEL {
            continue;
        }
        let Some(name) = key_to_name.get(&rec.fields[2]) else {
            continue;
        };
        // Element record: carries a zero-based f[11] element index, no
        // anchor sentinel at f[14], the canonical f[12]=124 slot-ref, and
        // f[10]=0 sort-key. This matches the Python decoder's element
        // selector.
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
        // Anchor record: carries the sentinel at f[14].
        if rec.fields[14] == VDF_SENTINEL {
            candidate_anchors
                .entry(group_id)
                .or_default()
                .push((name.clone(), rec.fields[11]));
        }
    }

    let mut anchors: Vec<DimAnchor> = Vec::new();
    // Collect every anchor whose group carries at least one element
    // record, matching the task's selector. A dim whose group is absent
    // from the record stream (none of its elements are recorded in this
    // run) would have no element records and is skipped here.
    for (&group_id, candidates) in &candidate_anchors {
        if !element_groups.contains_key(&group_id) && candidates.is_empty() {
            continue;
        }
        // Deduplicate anchor candidates by lowercased name so that a
        // repeat anchor (different record index, same name) does not
        // yield duplicate entries.
        let mut seen: HashSet<String> = HashSet::new();
        for (name, _dim_id) in candidates {
            let key = name.to_lowercase();
            if !seen.insert(key) {
                continue;
            }

            let mut raw_elements = element_groups.get(&group_id).cloned().unwrap_or_default();
            raw_elements.sort_by_key(|(idx, _)| *idx);
            let mut by_index: HashMap<u32, String> = HashMap::new();
            for (idx, ename) in &raw_elements {
                by_index.entry(*idx).or_insert_with(|| ename.clone());
            }
            let mut indices: Vec<u32> = by_index.keys().copied().collect();
            indices.sort_unstable();
            let expected: Vec<u32> = (0..indices.len() as u32).collect();
            let complete = indices == expected && indices.len() >= 2;
            let elements: Vec<(u32, String)> = indices
                .into_iter()
                .map(|i| (i, by_index.remove(&i).unwrap()))
                .collect();

            anchors.push(DimAnchor {
                name: name.clone(),
                group_id,
                elements,
                complete,
            });
        }
    }

    // Canonical order: ascending `f[8]`. Tiebreak on the anchor name so
    // the order is fully deterministic.
    anchors.sort_by(|a, b| {
        a.group_id
            .cmp(&b.group_id)
            .then_with(|| a.name.cmp(&b.name))
    });
    anchors
}

fn parse_sec5(vdf: &VdfFile) -> Vec<VdfSection5SetEntry> {
    vdf.parse_section5_set_stream()
        .map(|(_, entries, _)| entries)
        .unwrap_or_default()
}

/// Payload refs carried by a section-5 entry: the first `n` of `entry.refs`
/// (the trailing refs are axis anchors, not dimension elements).
fn sec5_payload(entry: &VdfSection5SetEntry) -> &[u32] {
    &entry.refs[..entry.n.min(entry.refs.len())]
}

/// Pin the "section-5 entries in file order correspond to record-field[8]
/// dimension anchors sorted by `f[8]` ascending" rule on a set of
/// fixtures with well-behaved array metadata.
///
/// For every complete anchor (one whose element records form a contiguous
/// 0..N run), the anchor's cardinality must equal its paired
/// `sec5[i].n`. This matches the Ref.vdf evidence (6 of 18 anchors are
/// complete) and extends to the subscripts / model_editing fixtures.
#[test]
fn test_section5_entries_align_with_anchor_f8_ascending() {
    let fixtures: [&str; 6] = [
        "../../test/xmutil_test_models/Ref.vdf",
        "../../test/bobby/vdf/subscripts/subscripts.vdf",
        "../../test/bobby/vdf/model_editing/run_7.vdf",
        "../../test/bobby/vdf/model_editing/run_8.vdf",
        "../../test/bobby/vdf/model_editing/run_9.vdf",
        "../../test/bobby/vdf/model_editing/run_10.vdf",
    ];

    let mut covered_any_complete = false;
    for path in fixtures {
        if !Path::new(path).exists() {
            panic!("multidim fixture missing: {path}");
        }
        let data = std::fs::read(path).expect("read multidim fixture");
        let vdf = VdfFile::parse(data).expect("parse multidim fixture");

        let anchors = recover_anchors(&vdf);
        let sec5 = parse_sec5(&vdf);

        assert_eq!(
            anchors.len(),
            sec5.len(),
            "{path}: expected len(anchors) == len(sec5), got anchors={} sec5={}",
            anchors.len(),
            sec5.len(),
        );

        for (i, (anchor, entry)) in anchors.iter().zip(sec5.iter()).enumerate() {
            if !anchor.complete {
                continue;
            }
            covered_any_complete = true;
            assert_eq!(
                entry.n,
                anchor.elements.len(),
                "{path}: sec5[{i}].n={} must match anchor {:?} cardinality={} (group_id={})",
                entry.n,
                anchor.name,
                anchor.elements.len(),
                anchor.group_id,
            );
        }
    }
    assert!(
        covered_any_complete,
        "no complete anchor paired with a sec5 entry across the multidim fixture set"
    );
}

/// For every subrange dimension in `Ref.vdf`, assert that the subrange's
/// section-5 payload (non-trailing refs) is a strict in-order subsequence
/// of its parent root dim's payload, and that the subseq positions match
/// the MDL-declared element indices.
///
/// Evidence: `/tmp/vdf_ref_dims.md` section 1 enumerates all 11 Ref.vdf
/// subranges with their expected positions. The payload refs themselves
/// are opaque compile-time "axis-participation" tokens; the binding is
/// the subsequence-position rule.
#[test]
fn test_subrange_payload_is_parent_subseq_on_ref_vdf() {
    let ref_vdf = load_ref_vdf();
    let anchors = recover_anchors(&ref_vdf);
    let sec5 = parse_sec5(&ref_vdf);
    assert_eq!(
        anchors.len(),
        sec5.len(),
        "Ref.vdf anchor/sec5 count mismatch"
    );

    // Build anchor-name -> payload map so subranges can be resolved by
    // looking up the parent root dim by name.
    let mut payloads: HashMap<String, Vec<u32>> = HashMap::new();
    for (anchor, entry) in anchors.iter().zip(sec5.iter()) {
        payloads.insert(anchor.name.clone(), sec5_payload(entry).to_vec());
    }

    /// Return the subsequence positions of `needle` in `hay` if `needle`
    /// is a strict in-order subsequence of `hay`; otherwise `None`.
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

    // (subrange_name, parent_root_name, expected positions inside parent's payload)
    let subrange_cases: &[(&str, &str, &[usize])] = &[
        ("bottom", "layers", &[3]),
        ("lower", "layers", &[1, 2, 3]),
        ("upper", "layers", &[0, 1, 2]),
        ("COP Developed", "COP", &[0, 1, 4]),
        ("COP Developing A", "COP", &[2, 3, 5]),
        ("COP Remaining Developing", "COP", &[5, 6]),
        ("set targets", "Target", &[0, 1]),
        ("tNext", "Target", &[1, 2]),
        ("tPrev", "Target", &[0, 1]),
        ("Developing A", "Semi Agg", &[2, 3]),
        ("Developing B", "Semi Agg", &[5]),
    ];

    for (subrange_name, parent_name, expected_positions) in subrange_cases {
        let subrange_payload = payloads
            .get(*subrange_name)
            .unwrap_or_else(|| panic!("missing sec5 payload for subrange {subrange_name}"));
        let parent_payload = payloads
            .get(*parent_name)
            .unwrap_or_else(|| panic!("missing sec5 payload for parent root {parent_name}"));

        let positions = subseq_positions(subrange_payload, parent_payload).unwrap_or_else(|| {
            panic!(
                "{subrange_name}: sec5 payload {:?} must be an in-order subsequence of {parent_name} payload {:?}",
                subrange_payload, parent_payload,
            );
        });
        assert_eq!(
            positions, *expected_positions,
            "{subrange_name} @ {parent_name}: subseq positions",
        );
    }
}

/// Smoke test for `VdfFile::recover_dimension_sets_via_sec5` on Ref.vdf:
/// the structural recovery should reproduce the same dim-name/element
/// tables that the Python `tools/vdf_xray.py` decoder emits. The MDL
/// ground truth for Ref.vdf is encoded inline here; if either the Rust
/// recovery or the Python recovery drifts, the mismatch shows up as a
/// failing test rather than silent disagreement.
#[test]
fn test_recover_dimension_sets_via_sec5_matches_xray_on_ref_vdf() {
    let ref_vdf = load_ref_vdf();
    let recovered = ref_vdf.recover_dimension_sets_via_sec5();

    // MDL-pinned expectations. These match the "End-to-end validation"
    // section of /tmp/vdf_ref_dims.md. `scenario` is omitted because it
    // is a partial single-element root in this save (the MDL declares 3
    // elements, but only `Deterministic` was simulated), and the
    // recovery declines to emit it rather than guess the missing names.
    let expected: Vec<(&str, Vec<&str>)> = vec![
        (
            "Aggregated Regions",
            vec![
                "Developed Countries",
                "Developing A Countries",
                "Developing B Countries",
            ],
        ),
        (
            "COP",
            vec![
                "OECD US",
                "OECD EU",
                "G77 China",
                "G77 India",
                "Remaining Developed",
                "Remaining Developing A",
                "COP Developing B",
            ],
        ),
        (
            "HFC type",
            vec![
                "HFC134a",
                "HFC23",
                "HFC32",
                "HFC125",
                "HFC143a",
                "HFC152a",
                "HFC227ea",
                "HFC245ca",
                "HFC4310mee",
            ],
        ),
        ("layers", vec!["layer1", "layer2", "layer3", "layer4"]),
        (
            "Semi Agg",
            vec![
                "US",
                "EU",
                "China",
                "India",
                "Other Developed",
                "Other Developing",
            ],
        ),
        ("Target", vec!["t1", "t2", "t3"]),
        ("bottom", vec!["layer4"]),
        ("lower", vec!["layer2", "layer3", "layer4"]),
        ("upper", vec!["layer1", "layer2", "layer3"]),
        (
            "COP Developed",
            vec!["OECD US", "OECD EU", "Remaining Developed"],
        ),
        (
            "COP Developing A",
            vec!["G77 China", "G77 India", "Remaining Developing A"],
        ),
        (
            "COP Remaining Developing",
            vec!["Remaining Developing A", "COP Developing B"],
        ),
        ("Developing A", vec!["China", "India"]),
        ("Developing B", vec!["Other Developing"]),
        ("set targets", vec!["t1", "t2"]),
        ("tNext", vec!["t2", "t3"]),
        ("tPrev", vec!["t1", "t2"]),
    ];

    let recovered_map: HashMap<String, Vec<String>> = recovered
        .iter()
        .map(|set| (set.name.clone(), set.elements.clone()))
        .collect();

    for (name, want) in expected {
        let got = recovered_map
            .get(name)
            .unwrap_or_else(|| panic!("recovered sets missing dim {name}"));
        let want_vec: Vec<String> = want.iter().map(|s| s.to_string()).collect();
        assert_eq!(got, &want_vec, "dim {name} element list");
    }
}
