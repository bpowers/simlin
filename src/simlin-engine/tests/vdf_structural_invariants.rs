// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Cross-corpus structural invariants pinned against the simulation VDF
//! fixture set. These tests replace prose "validated on N/N fixtures"
//! claims in `docs/design/vdf.md` with executable assertions that walk
//! every VDF under `test/`, `test/metasd/`, `test/xmutil_test_models/`,
//! and `third_party/uib_sd/zambaqui/`.
//!
//! Living in an integration-test file (not inside `src/vdf.rs`) keeps
//! the parser module under its 6000-line cap.

use std::path::{Path, PathBuf};

use simlin_engine::vdf::{VDF_FILE_MAGIC, VDF_SECTION6_OT_CODE_STOCK, VdfFile};

fn collect_vdf_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(collect_vdf_files(&path));
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("vdf"))
        {
            files.push(path);
        }
    }
    files
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn corpus_roots() -> &'static [&'static str] {
    &[
        "../../test/bobby/vdf",
        "../../test/metasd",
        "../../test/xmutil_test_models",
        "../../third_party/uib_sd/zambaqui",
    ]
}

/// Pin the three stable constants at the head of section-1 data across
/// the full simulation VDF corpus. Each is a direct word read at
/// `sec1.data_offset() + 0/4/8`:
///
/// - `data[0..4]` is a canonical base-slot constant (`124`), except on
///   WRLD3-03/SCEN01.VDF where it is `188`.
/// - `data[4..8]` equals `offset_table_count - 1 - max_stock_ot_index`,
///   where `max_stock_ot_index` is the largest OT index whose class code
///   is `VDF_SECTION6_OT_CODE_STOCK`.
/// - `data[8..12]` equals `section6_lookup_records().len()`.
#[test]
fn section1_data_head_structural_invariants() {
    let mut checked = 0usize;
    for root in corpus_roots() {
        let root_path = Path::new(root);
        if !root_path.exists() {
            continue;
        }
        for path in collect_vdf_files(root_path) {
            let Ok(data) = std::fs::read(&path) else {
                continue;
            };
            if data.len() < 4 || data[0..4] != VDF_FILE_MAGIC {
                continue;
            }
            let Ok(vdf) = VdfFile::parse(data) else {
                continue;
            };
            let Some(sec1) = vdf.sections.get(1) else {
                continue;
            };
            let base = sec1.data_offset();
            if base + 12 > vdf.data.len() {
                continue;
            }
            let d0 = read_u32_le(&vdf.data, base);
            let d4 = read_u32_le(&vdf.data, base + 4);
            let d8 = read_u32_le(&vdf.data, base + 8);

            let is_scen01 = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case("SCEN01.VDF"));
            let expected_d0 = if is_scen01 { 188 } else { 124 };
            assert_eq!(
                d0,
                expected_d0,
                "{}: sec1.data[0..4] canonical base-slot constant",
                path.display()
            );

            let lookup_count = vdf.section6_lookup_records().map(|r| r.len()).unwrap_or(0);
            assert_eq!(
                d8 as usize,
                lookup_count,
                "{}: sec1.data[8..12] must equal section6_lookup_records().len()",
                path.display()
            );

            let class_codes = vdf.section6_ot_class_codes().unwrap_or_default();
            let max_stock = class_codes
                .iter()
                .enumerate()
                .filter(|&(_, &c)| c == VDF_SECTION6_OT_CODE_STOCK)
                .map(|(i, _)| i)
                .max();
            if let Some(max_stock_idx) = max_stock {
                let expected_d4 = vdf
                    .offset_table_count
                    .saturating_sub(1)
                    .saturating_sub(max_stock_idx);
                assert_eq!(
                    d4 as usize,
                    expected_d4,
                    "{}: sec1.data[4..8] must equal OT_count - 1 - max_stock_ot_index",
                    path.display()
                );
            }

            checked += 1;
        }
    }

    assert!(
        checked >= 30,
        "expected to cross-check at least 30 simulation fixtures, got {checked}"
    );
}

/// Pin `skip_words = max(0, sec6.field4 - 1)` as the deterministic
/// formula for the section-6 ref-stream skip prefix. This is the
/// invariant that lets us retire the "try skip 0..=8 and pick the best
/// alignment" scan inside `parse_section6_ref_stream`.
///
/// Degenerate fixtures whose ref stream is empty do not constrain the
/// skip value (no entries are produced regardless of skip). On those we
/// only assert that the deterministic rule is self-consistent with
/// `sec6.field4 == 1`.
#[test]
fn section6_field4_matches_ref_stream_skip() {
    let mut checked = 0usize;
    for root in corpus_roots() {
        let root_path = Path::new(root);
        if !root_path.exists() {
            continue;
        }
        for path in collect_vdf_files(root_path) {
            let Ok(data) = std::fs::read(&path) else {
                continue;
            };
            if data.len() < 4 || data[0..4] != VDF_FILE_MAGIC {
                continue;
            }
            let Ok(vdf) = VdfFile::parse(data) else {
                continue;
            };
            let Some(sec6) = vdf.sections.get(6) else {
                continue;
            };
            let Some((skip, entries, _stop)) = vdf.parse_section6_ref_stream() else {
                continue;
            };

            let expected_skip = (sec6.field4 as usize).saturating_sub(1);

            if entries.is_empty() {
                assert_eq!(
                    expected_skip,
                    0,
                    "{}: degenerate fixture with sec6.field4 != 1",
                    path.display()
                );
                continue;
            }

            assert_eq!(
                skip,
                expected_skip,
                "{}: sec6.field4={} should imply skip_words={}, got {}",
                path.display(),
                sec6.field4,
                expected_skip,
                skip
            );
            checked += 1;
        }
    }

    assert!(
        checked >= 30,
        "expected to cross-check at least 30 simulation fixtures, got {checked}"
    );
}
