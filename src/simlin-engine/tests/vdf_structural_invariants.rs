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

use simlin_engine::vdf::{
    VDF_FILE_MAGIC, VDF_RECORD_VIEW_HEADER_CLASS, VDF_SECTION6_OT_CODE_STOCK, VdfFile,
};

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

/// Read the 16 u32 words of the section-1 "block 1" header (64 bytes at
/// `sec1.data_offset() + 76`). Returns `None` if the fixture cannot reach
/// that region cleanly.
fn read_sec1_block1(vdf: &VdfFile) -> Option<[u32; 16]> {
    let sec1 = vdf.sections.get(1)?;
    let base = sec1.data_offset();
    let block1_start = base + 76;
    let block1_end = block1_start + 64;
    if block1_end > vdf.data.len() {
        return None;
    }
    let mut words = [0u32; 16];
    for (i, word) in words.iter_mut().enumerate() {
        *word = read_u32_le(&vdf.data, block1_start + i * 4);
    }
    Some(words)
}

/// Pin the invariant relationship between `block1[10]` and `block1[11]`
/// across every simulation VDF in the corpus.
///
/// The two adjacent u32 words at `sec1.data_offset() + 116` and
/// `sec1.data_offset() + 120` carry a packed `(u16, u16)` flag pair where
/// the high 16 bits of word 10 always equal word 11. Cross-corpus
/// investigation (see the memory-regions audit) also found exactly three
/// distinct pair values. Pinning both the arithmetic relationship and the
/// observed value set makes a future change show up immediately rather
/// than drifting silently.
#[test]
fn test_block1_word10_high_equals_word11_across_corpus() {
    const OBSERVED_PAIRS: &[(u32, u32)] = &[(0, 0), (0x00600000, 96), (0x00f00000, 240)];

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
            let Some(block1) = read_sec1_block1(&vdf) else {
                continue;
            };
            let w10 = block1[10];
            let w11 = block1[11];
            assert_eq!(
                w10 >> 16,
                w11,
                "{}: block1[10]>>16 ({:#x}) must equal block1[11] ({:#x})",
                path.display(),
                w10 >> 16,
                w11,
            );
            assert!(
                OBSERVED_PAIRS.contains(&(w10, w11)),
                "{}: unexpected (block1[10], block1[11]) pair ({:#x}, {:#x}); corpus only carries {:?}",
                path.display(),
                w10,
                w11,
                OBSERVED_PAIRS,
            );
            checked += 1;
        }
    }

    assert!(
        checked >= 30,
        "expected to cross-check at least 30 simulation fixtures, got {checked}"
    );
}

/// Whether the Rust `find_slot_table` scanner is known to under-count
/// the slot table on this fixture. The Python `tools/vdf_xray.py` parser
/// consumes a broader leading-extra-slot layout and returns a larger
/// slot count that matches `block1[7]`; the Rust scanner uses a stricter
/// `min_stride >= 4` rule and misses a chunk of entries on these
/// specific files. The Python-observed delta (per the memory-regions
/// audit) is 0 on risk2 and 1 on SCEN01. The Rust under-count is
/// orthogonal to the format invariant tested here, so we explicitly
/// exempt those fixtures and track the parser gap separately.
fn rust_slot_table_undercount_known(path: &Path) -> bool {
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let parent = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("");
    matches!(
        (parent, file_name),
        ("econ", "risk2.vdf") | ("WRLD3-03", "SCEN01.VDF")
    )
}

/// Pin `block1[7]` as an "actively-written slot count" diagnostic that
/// is always within 2 of `slot_table.len()` on the `test/` corpus of
/// simulation VDFs.
///
/// This is *not* a pinned equality: the investigation found deltas of 0,
/// 1, and 2 across the corpus. The bound is diagnostic only, but having
/// it executable means any regression that breaks the relationship shows
/// up immediately.
///
/// Scope: the assertion runs across the `test/bobby/vdf`,
/// `test/metasd`, and `test/xmutil_test_models` roots. The zambaqui
/// third-party corpus is excluded from this test -- the audit notes
/// `zambaqui/old runs/Pop-6.vdf` has delta=-1, and additional zambaqui
/// files trigger a deeper Rust slot-finder under-count. Those
/// discrepancies indicate a Rust parser gap, not a format-level
/// violation.
///
/// A small set of in-scope fixtures is exempted: Rust's slot-table
/// scanner is known to return a short count on `econ/risk2.vdf` and
/// `WRLD3-03/SCEN01.VDF` because its `min_stride >= 4` rule rejects
/// valid leading-extra-slot layouts that the Python `vdf_xray` parser
/// accepts. On those fixtures the format-level invariant still holds
/// (block1[7] matches the true slot count); we flag the Rust parser
/// discrepancy separately rather than hide it behind a relaxed bound.
#[test]
fn test_block1_word7_matches_slot_count_within_small_delta() {
    const ROOTS: &[&str] = &[
        "../../test/bobby/vdf",
        "../../test/metasd",
        "../../test/xmutil_test_models",
    ];

    let mut checked = 0usize;
    let mut exempted = 0usize;
    let mut observed_deltas: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
    for root in ROOTS {
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
            let Some(block1) = read_sec1_block1(&vdf) else {
                continue;
            };
            let block1_word7 = block1[7] as i64;
            let slot_count = vdf.slot_table.len() as i64;
            let delta = slot_count - block1_word7;
            if rust_slot_table_undercount_known(&path) {
                exempted += 1;
                continue;
            }
            observed_deltas.insert(delta);
            assert!(
                delta.abs() <= 2,
                "{}: |slot_count - block1[7]| must be <= 2, got slot_count={} block1[7]={} delta={}",
                path.display(),
                slot_count,
                block1_word7,
                delta,
            );
            checked += 1;
        }
    }

    assert!(
        checked >= 30,
        "expected to cross-check at least 30 simulation fixtures, got {checked}"
    );
    // Exempted fixtures should still be present in the corpus so the
    // exemption list does not silently become a no-op.
    assert!(
        exempted >= 1,
        "expected at least one exempted fixture to still be tracked; found {exempted}"
    );

    // Record the observed delta set as part of the test output so that a
    // regression that shifts the distribution is visible without masking
    // it behind a strict equality assertion.
    let deltas: Vec<i64> = observed_deltas.into_iter().collect();
    assert!(
        deltas.iter().all(|d| d.abs() <= 2),
        "observed deltas outside the [-2, 2] diagnostic window: {deltas:?}",
    );
}

/// Assert that every simulation-result VDF fixture carries at least one
/// record with `field[1] == VDF_RECORD_VIEW_HEADER_CLASS` (138).
///
/// View header records mark view boundaries in the Vensim sketch. Their
/// existence is used by later decoding passes as a structural signal; the
/// audit recommendation (finding A.2.4) called for a Rust-level
/// cross-corpus existence check to pair with the documented prose claim.
#[test]
fn test_record_f1_138_view_header_exists_on_simulation_fixtures() {
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
            let view_header_count = vdf
                .records
                .iter()
                .filter(|r| r.fields[1] == VDF_RECORD_VIEW_HEADER_CLASS)
                .count();
            assert!(
                view_header_count >= 1,
                "{}: expected at least one view-header record (field[1]=={}), found {}",
                path.display(),
                VDF_RECORD_VIEW_HEADER_CLASS,
                view_header_count,
            );
            checked += 1;
        }
    }

    assert!(
        checked >= 30,
        "expected to cross-check at least 30 simulation fixtures, got {checked}"
    );
}

/// Across `test/bobby/vdf/bact/*.vdf` fixtures that share the same OT
/// shape (same model, same integrator, same tp_count), assert that the
/// 204-byte section-1 pre-record region (12-byte preamble + three 64-byte
/// header blocks) is structurally stable: at least 48 of its 51 u32 words
/// must be identical across every rerun. The investigation (memory-regions
/// audit section 4) pinned only three words as rerun-volatile:
/// `block0[14]`, `block0[15]`, and `block1[1]`, which vary together as
/// the triple `(N-1, N, N+1)`.
///
/// Only fixtures grouped by identical OT/record/slot counts can be
/// compared word-for-word; files such as `euler-10.vdf` (different
/// step count) or `rk4.vdf` (different integrator) differ in the
/// record layout itself and are excluded from the stability test.
#[test]
fn test_section1_preamble_and_block0_stable_across_bact_reruns() {
    let bact_dir = Path::new("../../test/bobby/vdf/bact");
    if !bact_dir.exists() {
        panic!("bact fixture directory missing: {}", bact_dir.display());
    }

    let mut fixtures: Vec<PathBuf> = collect_vdf_files(bact_dir);
    fixtures.sort();
    assert!(
        fixtures.len() >= 4,
        "expected at least four bact rerun fixtures, found {} in {}",
        fixtures.len(),
        bact_dir.display()
    );

    // Read the 51 u32 words of `(preamble + block0 + block1 + block2)`
    // for every bact file. Group files by their (ot, record, slot)
    // shape so we only compare true reruns. Skipping any file that fails
    // to parse would mask a real regression, so we fail hard on parse
    // errors inside the bact directory.
    type Shape = (usize, usize, usize);
    let mut groups: std::collections::BTreeMap<Shape, Vec<(PathBuf, [u32; 51])>> =
        std::collections::BTreeMap::new();
    for path in &fixtures {
        let data = std::fs::read(path)
            .unwrap_or_else(|err| panic!("read bact fixture {}: {err}", path.display()));
        assert!(
            data.len() >= 4 && data[0..4] == VDF_FILE_MAGIC,
            "{}: bact fixtures must carry the simulation VDF magic",
            path.display()
        );
        let vdf = VdfFile::parse(data)
            .unwrap_or_else(|err| panic!("parse bact fixture {}: {err}", path.display()));
        let sec1 = vdf
            .sections
            .get(1)
            .unwrap_or_else(|| panic!("{}: missing section 1", path.display()));
        let base = sec1.data_offset();
        let end = base + 204;
        assert!(
            end <= vdf.data.len(),
            "{}: section-1 pre-record region truncated",
            path.display()
        );
        let mut words = [0u32; 51];
        for (i, word) in words.iter_mut().enumerate() {
            *word = read_u32_le(&vdf.data, base + i * 4);
        }
        let shape: Shape = (
            vdf.offset_table_count,
            vdf.records.len(),
            vdf.slot_table.len(),
        );
        groups.entry(shape).or_default().push((path.clone(), words));
    }

    // The 204-byte region splits as `preamble (3 words) | block0 (16) |
    // block1 (16) | block2 (16)`. The rerun-volatile triple lives at
    // block0[14] (word index 17), block0[15] (word index 18), and
    // block1[1] (word index 20). Any additional drifting index beyond
    // those three is a red flag worth surfacing.
    const BLOCK0_14: usize = 3 + 14;
    const BLOCK0_15: usize = 3 + 15;
    const BLOCK1_1: usize = 3 + 16 + 1;

    // Largest rerun group. The memory-regions audit explicitly pinned the
    // `ot=7` group (Current.vdf, euler-1.vdf, euler-2.vdf, euler-5.vdf),
    // which this test must exercise. Any additional groups of size >= 2
    // are also validated, but the ot=7 group is the required anchor.
    let largest = groups
        .values()
        .map(|v| v.len())
        .max()
        .expect("at least one bact group");
    assert!(
        largest >= 4,
        "expected at least one bact rerun group with >= 4 members, got largest={largest}"
    );

    for (shape, group) in &groups {
        if group.len() < 2 {
            continue;
        }
        let baseline = group[0].1;
        let mut differing_indices: std::collections::BTreeSet<usize> =
            std::collections::BTreeSet::new();
        for (path, words) in &group[1..] {
            for (i, &w) in words.iter().enumerate() {
                if w != baseline[i] {
                    differing_indices.insert(i);
                    eprintln!(
                        "bact rerun diff at word[{i}] for shape {:?}: baseline {:#x} vs {} {:#x}",
                        shape,
                        baseline[i],
                        path.display(),
                        w
                    );
                }
            }
        }
        let stable = 51 - differing_indices.len();
        assert!(
            stable >= 48,
            "expected >= 48/51 stable words across bact rerun group {:?}, got {} (differing indices: {:?})",
            shape,
            stable,
            differing_indices
        );
        for idx in &differing_indices {
            assert!(
                matches!(*idx, BLOCK0_14 | BLOCK0_15 | BLOCK1_1),
                "bact rerun group {:?} drifted at unexpected word index {}; known volatile triple is {{{BLOCK0_14}, {BLOCK0_15}, {BLOCK1_1}}}",
                shape,
                idx,
            );
        }
    }
}
