// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Round-trip integration tests for the systems format writer.
//!
//! Each test parses a `.txt` systems file, translates it, writes it back
//! to systems format, re-parses the written output, re-translates, and
//! simulates -- comparing results against expected CSV output.

mod test_helpers;

use simlin_engine::Vm;
use simlin_engine::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};

use test_helpers::ensure_results;

/// Full round-trip: parse -> translate -> write -> parse -> translate -> simulate.
/// Compares simulation output against expected CSV data.
fn roundtrip_systems_file(txt_path: &str, csv_path: &str, rounds: u64) {
    eprintln!("round-trip: {txt_path}");

    let contents = std::fs::read_to_string(txt_path)
        .unwrap_or_else(|e| panic!("failed to read {txt_path}: {e}"));

    // First pass: parse -> translate -> write
    let model1 = simlin_engine::systems::parse(&contents)
        .unwrap_or_else(|e| panic!("failed to parse {txt_path}: {e}"));
    let project1 = simlin_engine::systems::translate::translate(&model1, rounds)
        .unwrap_or_else(|e| panic!("failed to translate {txt_path}: {e}"));
    let written = simlin_engine::compat::to_systems(&project1)
        .unwrap_or_else(|e| panic!("failed to write {txt_path}: {e}"));

    eprintln!("written output:\n{written}---");

    // Second pass: parse written output -> translate -> simulate
    let model2 = simlin_engine::systems::parse(&written).unwrap_or_else(|e| {
        panic!("failed to re-parse written output for {txt_path}: {e}\nwritten:\n{written}")
    });
    let project2 =
        simlin_engine::systems::translate::translate(&model2, rounds).unwrap_or_else(|e| {
            panic!("failed to re-translate written output for {txt_path}: {e}\nwritten:\n{written}")
        });

    // Simulate the round-tripped model
    let expected = simlin_engine::compat::load_csv(csv_path, b',')
        .unwrap_or_else(|e| panic!("failed to load {csv_path}: {e}"));
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project2, None);
    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap_or_else(|e| {
        panic!("VM compilation failed for round-tripped {txt_path}: {e:?}\nwritten:\n{written}")
    });
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| {
        panic!("VM creation failed for round-tripped {txt_path}: {e}\nwritten:\n{written}")
    });
    vm.run_to_end().unwrap_or_else(|e| {
        panic!("VM execution failed for round-tripped {txt_path}: {e}\nwritten:\n{written}")
    });
    let results = vm.into_results();
    ensure_results(&expected, &results);
}

#[test]
fn roundtrip_hiring() {
    roundtrip_systems_file(
        "../../test/systems-format/hiring.txt",
        "../../test/systems-format/hiring_output.csv",
        5,
    );
}

#[test]
fn roundtrip_links() {
    roundtrip_systems_file(
        "../../test/systems-format/links.txt",
        "../../test/systems-format/links_output.csv",
        5,
    );
}

#[test]
fn roundtrip_maximums() {
    roundtrip_systems_file(
        "../../test/systems-format/maximums.txt",
        "../../test/systems-format/maximums_output.csv",
        5,
    );
}

#[test]
fn roundtrip_projects() {
    roundtrip_systems_file(
        "../../test/systems-format/projects.txt",
        "../../test/systems-format/projects_output.csv",
        5,
    );
}

#[test]
fn roundtrip_extended_syntax() {
    roundtrip_systems_file(
        "../../test/systems-format/extended_syntax.txt",
        "../../test/systems-format/extended_syntax_output.csv",
        5,
    );
}

// Task 3: Compat API test
#[test]
fn compat_open_and_write_systems() {
    let contents = std::fs::read_to_string("../../test/systems-format/hiring.txt").unwrap();
    let project = simlin_engine::compat::open_systems(&contents).unwrap();
    let written = simlin_engine::compat::to_systems(&project).unwrap();

    // Verify written output can be re-parsed and compiled
    let project2 = simlin_engine::compat::open_systems(&written).unwrap();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project2, None);
    assert!(
        compile_project_incremental(&db, sync.project, "main").is_ok(),
        "round-tripped project should compile"
    );
}
