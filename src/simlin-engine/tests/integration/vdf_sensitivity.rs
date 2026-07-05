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
    VDF_SECTION6_OT_CODE_TIME, VDF_SENSITIVITY_FILE_MAGIC, VdfBlockGrid, VdfFile, VdfKind,
    is_owner_ot_class_code, probe_vdf_kind, read_u16,
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

/// Parse every zambaqui 0x53 file and validate its extraction against the
/// section-6 final-values oracle: the last extracted value of each OT series
/// must equal the file's own recorded final value (NaN-aware, mirroring
/// `test_section6_final_values_match_extracted_last_values`'s f32
/// tolerance). This cross-checks the whole decode chain -- offset table,
/// per-block bitmaps (including the header-0x74 data grid these files'
/// class-0x05 exogenous blocks live on, GH #842), inline constants -- on the
/// sensitivity container.
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

        // Every data block's bitmap must reconcile with one of the three
        // known grids (saved 0x78, block 0x7C, data 0x74). Before the
        // data-grid candidate existed, each of these files carried one
        // class-0x05 exogenous block (gdp deflator, 26 points on the
        // 26-point data grid) that reconciled with nothing and had to be
        // skipped from the oracle; that population is now decodable, so an
        // unreconciled block here is a regression or a genuinely new layout
        // and must fail loudly (GH #842).
        let unreconciled = vdf.unreconciled_data_blocks();
        assert!(
            unreconciled.is_empty(),
            "{display}: data blocks at OTs {unreconciled:?} reconcile with no known \
             bitmap grid (saved/block/data); their series would be NaN-filled"
        );
        // The pinned gdp-deflator slot must resolve onto the data grid
        // specifically -- the discriminator working by accident (e.g. a
        // saved-grid popcount coincidence) would silently mis-place values.
        if path.file_name().is_some_and(|n| n == "opt-1.vdf") {
            let raw = vdf
                .offset_table_entry(708)
                .expect("opt-1.vdf: OT[708] present");
            assert!(
                vdf.is_data_block_offset(raw),
                "opt-1.vdf: OT[708] is a block"
            );
            let count = read_u16(&vdf.data, raw as usize) as usize;
            let layout = vdf.block_bitmap_layout(raw as usize, count);
            assert_eq!(
                layout.grid,
                VdfBlockGrid::Data,
                "opt-1.vdf: OT[708] (gdp deflator) must decode on the header-0x74 data grid"
            );
            assert_eq!(layout.grid_count, 26, "opt-1.vdf: data grid count");
        }

        let final_values = vdf
            .section6_ot_final_values()
            .unwrap_or_else(|| panic!("{display}: missing section-6 final values"));
        let class_codes = vdf
            .section6_ot_class_codes()
            .unwrap_or_else(|| panic!("{display}: missing section-6 class codes"));
        let extracted = vdf
            .extract_data()
            .unwrap_or_else(|err| panic!("{display}: extract_data: {err}"));
        assert!(
            extracted.unreconciled_ots.is_empty(),
            "{display}: extract_data NaN-filled OTs {:?}",
            extracted.unreconciled_ots
        );

        assert_eq!(
            final_values.len(),
            extracted.entries.len(),
            "{display}: final-value vector length should match OT/data entries"
        );
        let mut checked = 0usize;
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
            let expected = series.last().copied().unwrap_or(f64::NAN) as f32;
            assert!(
                (final_value - expected).abs() < 1e-5
                    || (final_value.is_nan() && expected.is_nan()),
                "{display}: OT[{ot}] final value mismatch: parsed={final_value} expected={expected}"
            );
            checked += 1;
        }
        // Coverage floor: the real zambaqui 0x53 files check ~1,182-1,208
        // owner-coded slots each (opt-1.vdf: 194x0x08 stocks + 757x0x11
        // dynamics + 6x0x16 + 249x0x17 consts + the 0x05 data block + Time).
        // A weak `> 1` floor would let a class-code-parsing regression
        // quietly shrink the oracle to a handful of slots.
        assert!(
            checked >= 800,
            "{display}: final-values oracle only checked {checked} owner-coded \
             slots; expected at least 800"
        );

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
