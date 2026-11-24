// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The table `Results::print_tsv` prints, over a run.

use simlin_base::datamodel;

use crate::test_common::TestProject;

/// A standalone lookup table keeps a layout slot but has no key in the
/// offsets map (GH #606), so a run over one has more slots than names.
/// The TSV is the map's columns and nothing else: the header lists every
/// key in slot order and no other column, every row has that many cells,
/// and the comparison form prefixes a `series` column and pairs each
/// step's `reference` and `simlin` rows.
#[test]
fn tsv_prints_the_offset_maps_columns_and_no_unnamed_slot() {
    let table = datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 1.0, 2.0]),
        y_points: vec![0.0, 5.0, 10.0],
        x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 2.0 },
        y_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 10.0,
        },
    };
    let tp = TestProject::new("tsv_columns")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux_with_gf("table", "", table)
        .aux("y", "LOOKUP(table, TIME)", None);
    let compiled = tp.compile_incremental().expect("compiles");
    let mut vm = crate::vm::Vm::new(compiled).expect("vm");
    vm.run_to_end().expect("runs");
    let results = vm.into_results();
    assert!(
        results.step_size > results.offsets.len(),
        "the table's slot must be unnamed, or this run exercises nothing"
    );
    let mut expected: Vec<(String, usize)> = results
        .offsets
        .iter()
        .map(|(name, off)| (name.to_string(), *off))
        .collect();
    expected.sort_by_key(|(_, off)| *off);
    let expected_header: Vec<String> = expected.into_iter().map(|(name, _)| name).collect();
    assert!(expected_header.contains(&"y".to_string()));

    let mut out = Vec::new();
    results.write_tsv(&mut out, None).expect("writes");
    let text = String::from_utf8(out).expect("utf-8");
    let rows: Vec<Vec<&str>> = text.lines().map(|l| l.split('\t').collect()).collect();
    assert_eq!(
        rows[0], expected_header,
        "the header is the map's keys in slot order"
    );
    assert_eq!(rows.len(), 1 + results.step_count, "one row per saved step");
    for row in &rows[1..] {
        assert_eq!(
            row.len(),
            expected_header.len(),
            "every row has one cell per column"
        );
    }
    let y = expected_header
        .iter()
        .position(|n| n == "y")
        .expect("y column");
    assert_eq!(rows[2][y], "5", "y = LOOKUP(table, 1) at step 1");

    let mut out = Vec::new();
    results.write_tsv(&mut out, Some(&results)).expect("writes");
    let text = String::from_utf8(out).expect("utf-8");
    let rows: Vec<Vec<&str>> = text.lines().map(|l| l.split('\t').collect()).collect();
    let mut header = vec!["series".to_string()];
    header.extend(expected_header.iter().cloned());
    assert_eq!(rows[0], header);
    assert_eq!(rows.len(), 1 + 2 * results.step_count);
    for pair in rows[1..].chunks(2) {
        assert_eq!(pair[0][0], "reference");
        assert_eq!(pair[1][0], "simlin");
        assert_eq!(
            pair[0][1..],
            pair[1][1..],
            "a run compared with itself agrees"
        );
    }
}
