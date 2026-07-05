// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! 0x53 sensitivity/optimization-run VDF coverage.
//!
//! Sensitivity runs (`VDF_SENSITIVITY_FILE_MAGIC`, `7f f7 17 53`) carry the
//! same eight-section layout as ordinary 0x52 run files plus an undecoded
//! payload past the sparse-block run (anchored by header word 0x68). The
//! reader follows OT offsets, so it never touches that tail; parsing and
//! extraction work with the 0x52 rules. See docs/design/vdf.md,
//! "Sensitivity / optimization format".
//!
//! Fixtures live in `third_party/uib_sd/zambaqui` (the only corpus source of
//! 0x53 files), so this module uses the same existence-continue convention as
//! `vdf_structural_invariants.rs`: skip when the directory is absent, but if
//! any 0x53 file IS present it must parse and satisfy the section-6
//! final-values oracle.

use std::path::{Path, PathBuf};

use simlin_engine::vdf::{
    VDF_SECTION6_OT_CODE_TIME, VDF_SENSITIVITY_FILE_MAGIC, VdfFile, VdfKind,
    is_owner_ot_class_code, probe_vdf_kind, read_u16,
};

/// Whether the data block at `block_offset` carries a bitmap the reader can
/// actually reconcile: its declared value count must equal the popcount of
/// the bitmap bytes under one of the two known grid widths (saved grid or
/// full block grid). The zambaqui corpus contains class-0x05 exogenous-data
/// blocks stored on their OWN short time grid (`gdp deflator`: 26 points on
/// a 71-point run), which neither reader decodes yet -- manual inspection
/// showed both fall back to the run-grid bitmap on these files (no automated
/// test pins 0x53 Rust-vs-Python parity; the parity harness corpus is
/// `test/` only) -- so the final-values oracle cannot hold there. This is a
/// pre-existing limitation on zambaqui 0x52 files too, not a 0x53 artifact;
/// tracked as GH #842.
fn block_bitmap_is_decodable(vdf: &VdfFile, block_offset: usize) -> bool {
    if block_offset + 2 > vdf.data.len() {
        return false;
    }
    let count = read_u16(&vdf.data, block_offset) as usize;
    let (bitmap_size, _grid) = vdf.block_bitmap_layout(block_offset, count);
    let bm_start = block_offset + 2;
    let bm_end = bm_start + bitmap_size;
    if bm_end > vdf.data.len() {
        return false;
    }
    let popcount: usize = vdf.data[bm_start..bm_end]
        .iter()
        .map(|b| b.count_ones() as usize)
        .sum();
    popcount == count
}

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

/// Parse every zambaqui 0x53 file and validate its extraction against the
/// section-6 final-values oracle: the last extracted value of each OT series
/// must equal the file's own recorded final value (NaN-aware, mirroring
/// `test_section6_final_values_match_extracted_last_values`'s f32
/// tolerance). This cross-checks the whole decode chain -- offset table,
/// per-block bitmaps, inline constants -- on the sensitivity container.
#[test]
fn sensitivity_run_files_parse_and_match_final_values_oracle() {
    let root = Path::new("../../third_party/uib_sd/zambaqui");
    if !root.exists() {
        // third_party corpora are optional checkouts; absent means skip
        // (matching vdf_structural_invariants.rs's corpus_roots convention).
        return;
    }

    let sensitivity_files: Vec<PathBuf> = collect_vdf_files(root)
        .into_iter()
        .filter(|path| {
            std::fs::read(path)
                .map(|data| data.len() >= 4 && data[0..4] == VDF_SENSITIVITY_FILE_MAGIC)
                .unwrap_or(false)
        })
        .collect();

    // Pin a known fixture by name: if the zambaqui checkout exists, the 0x53
    // population must not silently vanish (an empty filter would otherwise
    // make this test pass vacuously).
    assert!(
        sensitivity_files
            .iter()
            .any(|p| p.file_name().is_some_and(|n| n == "opt-1.vdf")),
        "zambaqui checkout exists but the pinned 0x53 fixture opt-1.vdf is missing \
         (found {} sensitivity files: {:?})",
        sensitivity_files.len(),
        sensitivity_files,
    );

    for path in &sensitivity_files {
        let display = path.display();
        let data = std::fs::read(path).unwrap_or_else(|err| panic!("{display}: read: {err}"));
        assert_eq!(
            probe_vdf_kind(&data),
            Some(VdfKind::SensitivityRun),
            "{display}: expected the sensitivity-run probe kind"
        );

        let vdf = VdfFile::parse(data)
            .unwrap_or_else(|err| panic!("{display}: 0x53 file must parse: {err}"));

        let final_values = vdf
            .section6_ot_final_values()
            .unwrap_or_else(|| panic!("{display}: missing section-6 final values"));
        let class_codes = vdf
            .section6_ot_class_codes()
            .unwrap_or_else(|| panic!("{display}: missing section-6 class codes"));
        let extracted = vdf
            .extract_data()
            .unwrap_or_else(|err| panic!("{display}: extract_data: {err}"));

        assert_eq!(
            final_values.len(),
            extracted.entries.len(),
            "{display}: final-value vector length should match OT/data entries"
        );
        let mut checked = 0usize;
        let mut skipped: Vec<(usize, u8)> = Vec::new();
        for (ot, (final_value, series)) in final_values
            .iter()
            .zip(extracted.entries.iter())
            .enumerate()
        {
            // Optimization/sensitivity runs save only a subset of the model's
            // variables: unsaved OT slots carry class code 0 with a zero
            // offset-table word, and their section-6 final value is Vensim's
            // :NA: sentinel (-1.298e33) rather than a real last value. Those
            // slots are never claimed by decoded record spans (the class-code
            // guard rejects non-owner codes), so the oracle only applies to
            // Time and owner-coded slots.
            let code = class_codes.get(ot).copied().unwrap_or(0);
            if code != VDF_SECTION6_OT_CODE_TIME && !is_owner_ot_class_code(code) {
                continue;
            }
            // Skip blocks whose bitmap layout the reader knowably cannot
            // reconcile (see block_bitmap_is_decodable). This is a
            // structural gate, not a name-based exclusion list, and it is
            // bounded below so it cannot quietly swallow the whole oracle.
            if let Some(raw) = vdf.offset_table_entry(ot)
                && vdf.is_data_block_offset(raw)
                && !block_bitmap_is_decodable(&vdf, raw as usize)
            {
                skipped.push((ot, code));
                continue;
            }
            let expected = series.last().copied().unwrap_or(f64::NAN) as f32;
            assert!(
                (final_value - expected).abs() < 1e-5
                    || (final_value.is_nan() && expected.is_nan()),
                "{display}: OT[{ot}] final value mismatch: parsed={final_value} expected={expected}"
            );
            checked += 1;
        }
        // Coverage floor: the real zambaqui 0x53 files check ~1,181-1,207
        // owner-coded slots each (opt-1.vdf: 194x0x08 stocks + 757x0x11
        // dynamics + 6x0x16 + 249x0x17 consts + Time, minus the one skipped
        // block). A weak `> 1` floor would let a class-code-parsing
        // regression quietly shrink the oracle to a handful of slots.
        assert!(
            checked >= 800,
            "{display}: final-values oracle only checked {checked} owner-coded \
             slots; expected at least 800"
        );
        // The known undecodable population is the single own-grid exogenous
        // data block (gdp deflator, class 0x05; GH #842). Bound the skips by
        // IDENTITY, not just cardinality: every skipped slot must be a
        // class-0x05 data block, and for the pinned fixtures the exact OT
        // set is known -- a regression that swaps WHICH blocks are
        // undecodable must fail loudly even if the count stays small.
        assert!(
            skipped.len() <= 2,
            "{display}: {} owner-coded blocks skipped as bitmap-undecodable \
             ({skipped:?}); expected at most 2",
            skipped.len()
        );
        for &(ot, code) in &skipped {
            assert_eq!(
                code, 0x05,
                "{display}: skipped OT[{ot}] has class code {code:#04x}; only \
                 class-0x05 exogenous-data blocks are known to be undecodable"
            );
        }
        let skipped_ots: Vec<usize> = skipped.iter().map(|&(ot, _)| ot).collect();
        let expected_skips: Option<&[usize]> = match path.file_name().and_then(|n| n.to_str()) {
            Some("opt-1.vdf") | Some("sens-train_cost.vdf") => Some(&[708]),
            Some("sensi-1.vdf") => Some(&[696]),
            _ => None,
        };
        if let Some(expected) = expected_skips {
            assert_eq!(
                skipped_ots, expected,
                "{display}: bitmap-undecodable OT set drifted from the pinned \
                 gdp-deflator slot"
            );
        }

        // The record-driven name mapping must produce real columns too (Time
        // plus at least one model variable) -- the sensitivity container is
        // supposed to be a full result file, not just structurally parseable.
        let results = vdf
            .to_results_via_records()
            .unwrap_or_else(|err| panic!("{display}: to_results_via_records: {err}"));
        assert!(
            results.offsets.len() > 1,
            "{display}: expected named result columns beyond Time, got {}",
            results.offsets.len()
        );
    }
}
