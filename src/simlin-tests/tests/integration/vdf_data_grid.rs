// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Header-0x74 data-grid bitmap coverage (GH #842).
//!
//! Exogenous-data blocks (class codes 0x05/0x06/0x0c) store their bitmaps on
//! the external data file's time grid, whose point count is header word 0x74.
//! Before that third candidate existed the reader silently fell back to the
//! block-grid width and decoded garbage. These tests pin the decoded values
//! against the section-6 final-value oracle on the always-present `test/`
//! fixtures (the metasd groupon family, 6-point data grid on a 121-point
//! run) and, behind the existence-continue convention, on the zambaqui
//! third_party corpus (26- and 71-point data grids); plus the loud NaN-fill
//! fallback for a block that reconciles with no known grid.

use std::path::Path;

use simlin_engine::vdf::{VdfBlockGrid, VdfFile, read_u16};

fn vdf_file(path: &str) -> VdfFile {
    let data = std::fs::read(path).unwrap_or_else(|err| panic!("{path}: read: {err}"));
    VdfFile::parse(data).unwrap_or_else(|err| panic!("{path}: parse: {err}"))
}

/// Assert that every data block in the file reconciles with a known grid and
/// that every owner-coded OT's extracted series ends at the file's recorded
/// final value. Returns the OTs that resolved onto the data grid so callers
/// can pin the expected population.
fn assert_all_blocks_decodable(path: &str, vdf: &VdfFile) -> Vec<usize> {
    assert!(
        vdf.unreconciled_data_blocks().is_empty(),
        "{path}: blocks reconcile with no known bitmap grid: {:?}",
        vdf.unreconciled_data_blocks()
    );

    let final_values = vdf
        .section6_ot_final_values()
        .unwrap_or_else(|| panic!("{path}: missing section-6 final values"));
    let extracted = vdf
        .extract_data()
        .unwrap_or_else(|err| panic!("{path}: extract_data: {err}"));
    assert!(
        extracted.unreconciled_ots.is_empty(),
        "{path}: extract_data NaN-filled OTs {:?}",
        extracted.unreconciled_ots
    );

    let mut data_grid_ots = Vec::new();
    for (ot, final_value) in final_values.iter().enumerate() {
        let Some(raw) = vdf.offset_table_entry(ot) else {
            continue;
        };
        if !vdf.is_data_block_offset(raw) {
            continue;
        }
        let count = read_u16(&vdf.data, raw as usize) as usize;
        let layout = vdf.block_bitmap_layout(raw as usize, count);
        assert_ne!(
            layout.grid,
            VdfBlockGrid::Unreconciled,
            "{path}: OT[{ot}] bitmap unreconciled"
        );
        if layout.grid == VdfBlockGrid::Data {
            data_grid_ots.push(ot);
        }
        // Every DATA-BLOCK series must end at the recorded final value --
        // this is the oracle that fails when a bitmap width is misdecoded
        // in the too-narrow direction (values shifted into the bitmap).
        let series = &extracted.entries[ot];
        let expected = series.last().copied().unwrap_or(f64::NAN) as f32;
        assert!(
            (final_value - expected).abs() < 1e-5 * final_value.abs().max(1.0)
                || (final_value.is_nan() && expected.is_nan()),
            "{path}: OT[{ot}] final value mismatch: recorded={final_value} extracted={expected}"
        );
    }
    data_grid_ots
}

/// The metasd groupon fixtures (always present under `test/`) carry 21
/// exogenous-data blocks each on a 6-point data grid (header 0x74=6) inside
/// a 121-point monthly run. Before the data-grid candidate these blocks
/// silently misdecoded: their 1-byte bitmaps were read with the 16-byte
/// run-grid width, pulling payload bytes into the bitmap.
#[test]
fn groupon_data_grid_blocks_decode() {
    let fixtures = [
        "../../test/metasd/social-network-valuation/groupon3mid.vdf",
        "../../test/metasd/social-network-valuation/groupon3opt.vdf",
        "../../test/metasd/social-network-valuation/groupon3pess.vdf",
        "../../test/metasd/social-network-valuation/groupon3worst.vdf",
        "../../test/metasd/social-network-valuation/optimistic.vdf",
        "../../test/metasd/social-network-valuation/pessimistic.vdf",
    ];
    for path in fixtures {
        let vdf = vdf_file(path);
        assert_eq!(vdf.data_time_point_count, 6, "{path}: header 0x74");
        assert_eq!(vdf.data_bitmap_size, 1, "{path}: data-grid bitmap width");
        let data_grid_ots = assert_all_blocks_decodable(path, &vdf);
        assert_eq!(
            data_grid_ots.len(),
            21,
            "{path}: expected 21 data-grid blocks, found {data_grid_ots:?}"
        );
    }

    // Value-level pin on one block: groupon3mid OT[53] stores 4 values at
    // data-grid points {1,2,3,5} (bitmap 0x2e). The saved series must start
    // NaN (no data before the first stored point), end at the recorded
    // final value, and contain exactly the four stored values.
    let path = "../../test/metasd/social-network-valuation/groupon3mid.vdf";
    let vdf = vdf_file(path);
    let raw = vdf.offset_table_entry(53).unwrap();
    let count = read_u16(&vdf.data, raw as usize) as usize;
    assert_eq!(count, 4);
    let layout = vdf.block_bitmap_layout(raw as usize, count);
    assert_eq!(layout.grid, VdfBlockGrid::Data);
    assert_eq!(layout.grid_count, 6);
    let extracted = vdf.extract_data().unwrap();
    let series = &extracted.entries[53];
    assert!(series[0].is_nan(), "no data before the first stored point");
    assert!((series.last().unwrap() - 29_504_314.0).abs() < 1.0);
    let mut distinct: Vec<f64> = series.iter().copied().filter(|v| !v.is_nan()).collect();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        4,
        "{path}: OT[53] should surface exactly its 4 stored values, got {distinct:?}"
    );
    assert!((distinct[0] - 375_099.0).abs() < 1.0, "first stored value");
}

/// zambaqui corpus sweep (existence-continue: third_party checkouts are
/// optional): every 0x52/0x53 run file's data blocks must reconcile, and the
/// known data-grid populations must resolve onto the header-0x74 grid --
/// `baserun.vdf` (26-point data grid inside a 71-point yearly run) and
/// `old runs/Current.vdf` (71-point yearly data grid inside a 561-point
/// eighth-year run, where interior placement is additionally pinned by exact
/// block tiling).
#[test]
fn zambaqui_data_grid_blocks_decode() {
    let root = Path::new("../../third_party/uib_sd/zambaqui");
    if !root.exists() {
        return;
    }

    // Corpus sweep: no run file may contain an unreconciled block.
    let mut run_files = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if !path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("vdf"))
            {
                continue;
            }
            let Ok(data) = std::fs::read(&path) else {
                continue;
            };
            let Ok(vdf) = VdfFile::parse(data) else {
                // Data.vdf is the dataset sibling (0x41) and is expected to
                // be rejected by the run-file parser.
                continue;
            };
            run_files += 1;
            assert!(
                vdf.unreconciled_data_blocks().is_empty(),
                "{}: unreconciled blocks {:?}",
                path.display(),
                vdf.unreconciled_data_blocks()
            );
        }
    }
    assert!(
        run_files >= 50,
        "zambaqui checkout exists but only {run_files} run files parsed \
         (an empty walk must fail loudly)"
    );

    // gdp deflator in baserun.vdf: 26 values on the 26-point data grid
    // (dense bitmap `ff ff ff 03`), byte-identical to the sibling Data.vdf
    // dataset block. First/last values are ground truth from Data.vdf.
    let path = "../../third_party/uib_sd/zambaqui/baserun.vdf";
    let vdf = vdf_file(path);
    assert_eq!(vdf.data_time_point_count, 26, "{path}: header 0x74");
    let raw = vdf.offset_table_entry(696).unwrap();
    let count = read_u16(&vdf.data, raw as usize) as usize;
    assert_eq!(count, 26);
    let layout = vdf.block_bitmap_layout(raw as usize, count);
    assert_eq!(layout.grid, VdfBlockGrid::Data);
    assert_eq!((layout.bitmap_size, layout.grid_count), (4, 26));
    let extracted = vdf.extract_data().unwrap();
    let series = &extracted.entries[696];
    // Extraction only widens the stored f32s to f64, so the ground-truth
    // pins are exact-equality (the literals are the widened block bytes).
    assert_eq!(series[0], 0.6202020049095154, "{path}: OT[696] first value");
    assert_eq!(
        *series.last().unwrap(),
        2.086980104446411,
        "{path}: OT[696] last value"
    );
    assert_all_blocks_decodable(path, &vdf);

    // The same deflator series in the "old runs" family lives at data-grid
    // points 0..25 of a 71-point yearly grid (bitmap 9 bytes), inside a
    // 561-point run whose saved/block bitmaps are 71 BYTES -- the case where
    // the data grid is a genuinely third width, pinned by exact block
    // tiling against the next block's offset.
    let path = "../../third_party/uib_sd/zambaqui/old runs/Current.vdf";
    let vdf = vdf_file(path);
    assert_eq!(vdf.time_point_count, 561, "{path}: saved grid");
    assert_eq!(vdf.bitmap_size, 71, "{path}: saved bitmap bytes");
    assert_eq!(vdf.data_time_point_count, 71, "{path}: header 0x74");
    assert_eq!(vdf.data_bitmap_size, 9, "{path}: data-grid bitmap bytes");
    let raw = vdf.offset_table_entry(36).unwrap();
    let count = read_u16(&vdf.data, raw as usize) as usize;
    assert_eq!(count, 26, "{path}: gdp deflator stores 26 values");
    let layout = vdf.block_bitmap_layout(raw as usize, count);
    assert_eq!(layout.grid, VdfBlockGrid::Data);
    assert_eq!((layout.bitmap_size, layout.grid_count), (9, 71));
    let extracted = vdf.extract_data().unwrap();
    let series = &extracted.entries[36];
    assert_eq!(series[0], 0.6202020049095154, "{path}: OT[36] first value");
    assert_eq!(
        *series.last().unwrap(),
        2.086980104446411,
        "{path}: OT[36] last value"
    );
    // Interior placement: the data grid spans the run yearly, so the value
    // one saved step (0.125yr) after 1980 must still be the 1980 value, and
    // the value at 1981.0 must be the second stored value (zero-order hold
    // on the yearly grid).
    assert_eq!(
        series[1], series[0],
        "{path}: ZOH within the first data year"
    );
    assert_eq!(
        series[8], 0.6973530054092407,
        "{path}: value at 1981.0 should be the second stored value"
    );
    assert_all_blocks_decodable(path, &vdf);
}

/// Loud fallback: a block whose bitmap reconciles with NO known grid must be
/// NaN-filled and reported in `VdfData::unreconciled_ots` /
/// `unreconciled_data_blocks()`, never decoded under an assumed width.
/// Synthesized by zeroing a real dynamic block's bitmap in a small fixture
/// (popcount 0 can never equal a nonzero count).
#[test]
fn unreconciled_block_is_nan_filled_and_reported() {
    let path = "../../test/bobby/vdf/water/Current.vdf";
    let mut data = std::fs::read(path).unwrap();
    let vdf = VdfFile::parse(data.clone()).unwrap();
    assert_eq!(vdf.data_time_point_count, 0, "water has no data grid");
    // OT[5] is a dynamic (0x11) data block in this fixture.
    let raw = vdf.offset_table_entry(5).unwrap() as usize;
    assert!(vdf.is_data_block_offset(raw as u32));
    let count = read_u16(&vdf.data, raw) as usize;
    assert!(count > 0);
    for b in &mut data[raw + 2..raw + 2 + vdf.bitmap_size] {
        *b = 0;
    }

    let corrupted = VdfFile::parse(data).unwrap();
    let count = read_u16(&corrupted.data, raw) as usize;
    let layout = corrupted.block_bitmap_layout(raw, count);
    assert_eq!(layout.grid, VdfBlockGrid::Unreconciled);
    assert_eq!(corrupted.unreconciled_data_blocks(), vec![5]);

    let extracted = corrupted.extract_data().unwrap();
    assert_eq!(extracted.unreconciled_ots, vec![5]);
    assert!(
        extracted.entries[5].iter().all(|v| v.is_nan()),
        "unreconciled block must be NaN-filled, got {:?}",
        &extracted.entries[5][..4]
    );
    // Other blocks are unaffected.
    assert!(extracted.entries[1].iter().all(|v| !v.is_nan()));
}
