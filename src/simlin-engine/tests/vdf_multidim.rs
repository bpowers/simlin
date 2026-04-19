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

use simlin_engine::vdf::VdfFile;

fn load_ref_vdf() -> VdfFile {
    let data =
        std::fs::read("../../test/xmutil_test_models/Ref.vdf").expect("read Ref.vdf fixture");
    VdfFile::parse(data).expect("parse Ref.vdf")
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
