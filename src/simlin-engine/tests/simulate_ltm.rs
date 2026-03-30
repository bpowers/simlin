// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use std::result::Result as StdResult;

use simlin_engine::common::{Canonical, Ident};
use simlin_engine::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};
use simlin_engine::db::{set_project_ltm_discovery_mode, set_project_ltm_enabled};
use simlin_engine::xmile;
use simlin_engine::{CompiledSimulation, Project, Results, Vm, json, ltm, ltm_finding};

const LTM_TOLERANCE: f64 = 0.05;

/// Compile a datamodel project to a VM simulation using the incremental
/// salsa path with LTM enabled (exhaustive mode).
fn compile_ltm_incremental(project: &simlin_engine::datamodel::Project) -> CompiledSimulation {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    compile_project_incremental(&db, sync.project, "main").unwrap()
}

/// Compile a datamodel project to a VM simulation using the incremental
/// salsa path with LTM in discovery mode (scores for every causal edge).
fn compile_ltm_discovery_incremental(
    project: &simlin_engine::datamodel::Project,
) -> CompiledSimulation {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    compile_project_incremental(&db, sync.project, "main").unwrap()
}

struct LtmResults {
    loop_scores: HashMap<String, Vec<(f64, f64)>>,
}

fn load_ltm_results(file_path: &str) -> StdResult<LtmResults, Box<dyn Error>> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .from_path(file_path)?;

    let header = rdr.headers()?;

    // The reference data appears to be shifted by 1 DT to the left compared to our output.
    // Values at reference t=N match our calculations at t=N+1.
    // We shift the reference timestamps forward by 1 when loading.
    let dt = 1.0; // DT from the logistic growth model

    let mut times: Vec<f64> = Vec::new();
    for (i, field) in header.iter().enumerate() {
        if i == 0 {
            continue;
        }
        use std::str::FromStr;
        let time = f64::from_str(field.trim())?;
        times.push(time + dt);
    }

    let mut loop_scores: HashMap<String, Vec<(f64, f64)>> = HashMap::new();

    for result in rdr.records() {
        let record = result?;
        let loop_id = record[0].to_string();

        let mut scores: Vec<(f64, f64)> = Vec::new();
        for (i, field) in record.iter().enumerate() {
            if i == 0 {
                continue;
            }

            let value_str = field.trim();
            let value = if let Some(num_str) = value_str.strip_suffix('%') {
                use std::str::FromStr;
                f64::from_str(num_str)? / 100.0
            } else {
                use std::str::FromStr;
                f64::from_str(value_str)? / 100.0
            };

            let time = times[i - 1];
            scores.push((time, value));
        }

        loop_scores.insert(loop_id, scores);
    }

    Ok(LtmResults { loop_scores })
}

fn ensure_ltm_results(expected: &LtmResults, actual_results: &Results, loops: &[ltm::Loop]) {
    let mut errors = Vec::new();

    for (loop_id, expected_scores) in &expected.loop_scores {
        let var_name = format!("$⁚ltm⁚rel_loop_score⁚{}", loop_id);
        let var_ident =
            Ident::<Canonical>::from_str_unchecked(&Ident::new(&var_name).to_source_repr());

        if !actual_results.offsets.contains_key(&var_ident) {
            panic!("LTM results missing loop score variable '{}'", var_name);
        }

        let var_offset = actual_results.offsets[&var_ident];
        let mut loop_errors = Vec::new();
        let mut actual_series = Vec::new();

        for (expected_time, expected_value) in expected_scores {
            if *expected_time < actual_results.specs.start
                || *expected_time > actual_results.specs.stop
            {
                continue;
            }

            let mut found_match = false;

            for (step, result_row) in actual_results.iter().enumerate() {
                let time =
                    actual_results.specs.start + actual_results.specs.save_step * (step as f64);

                if (time - expected_time).abs() < 1e-9 {
                    found_match = true;
                    let actual_value = result_row[var_offset];
                    actual_series.push((time, actual_value));

                    // Skip t=1 comparison - at initialization we don't have enough history
                    // for meaningful link scores (need PREVIOUS values)
                    if (time - 1.0).abs() < 1e-9 {
                        break;
                    }

                    let max_abs = expected_value.abs().max(1e-10);
                    let relative_error = (expected_value - actual_value).abs() / max_abs;

                    if relative_error > LTM_TOLERANCE {
                        loop_errors.push((time, *expected_value, actual_value, relative_error));
                    }
                    break;
                }
            }

            if !found_match {
                panic!(
                    "Could not find timestep {} in simulation results for loop {}",
                    expected_time, loop_id
                );
            }
        }

        if !loop_errors.is_empty() {
            errors.push((
                loop_id.clone(),
                expected_scores.clone(),
                actual_series,
                loop_errors,
            ));
        }
    }

    if !errors.is_empty() {
        eprintln!("\n========================================");
        eprintln!("LTM RESULT MISMATCHES DETECTED");
        eprintln!("========================================\n");

        for (loop_id, expected_series, actual_series, point_errors) in &errors {
            let loop_info = loops.iter().find(|l| l.id == *loop_id);

            eprintln!("Loop: {}", loop_id);
            if let Some(loop_obj) = loop_info {
                eprintln!(
                    "  Polarity: {}",
                    match loop_obj.polarity {
                        ltm::LoopPolarity::Reinforcing => "Reinforcing (R)",
                        ltm::LoopPolarity::Balancing => "Balancing (B)",
                        ltm::LoopPolarity::Undetermined => "Undetermined (U)",
                    }
                );
                eprintln!("  Path: {}", loop_obj.format_path());
            }
            eprintln!(
                "  {} time points with errors (tolerance: {:.1}%)",
                point_errors.len(),
                LTM_TOLERANCE * 100.0
            );
            eprintln!("\n  Expected time series:");
            for (time, value) in expected_series {
                eprintln!("    t={:6.2}: {:8.4}", time, value);
            }
            eprintln!("\n  Actual time series:");
            for (time, value) in actual_series {
                eprintln!("    t={:6.2}: {:8.4}", time, value);
            }
            eprintln!("\n  Specific errors:");
            for (time, expected, actual, rel_err) in point_errors {
                eprintln!(
                    "    t={:6.2}: expected {:8.4}, got {:8.4} (relative error: {:.2}%)",
                    time,
                    expected,
                    actual,
                    rel_err * 100.0
                );
            }
            eprintln!();
        }

        panic!(
            "LTM verification failed with {} loop(s) having mismatched results",
            errors.len()
        );
    }
}

fn simulate_ltm_path(model_path: &str) {
    let f = File::open(model_path).unwrap();
    let mut f = BufReader::new(f);
    let datamodel_project = xmile::project_from_reader(&mut f).unwrap();

    // VM path via incremental compilation with LTM enabled
    let compiled = compile_ltm_incremental(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Project::from for structural loop detection (error reporting in ensure_ltm_results)
    let project = Project::from(datamodel_project);
    let main_ident: Ident<Canonical> = Ident::new("main");
    let loops = ltm::detect_loops(&project.models[&main_ident], &project).unwrap();

    let xmile_name = std::path::Path::new(model_path).file_name().unwrap();
    let dir_path = &model_path[0..(model_path.len() - xmile_name.len())];
    let dir_path = std::path::Path::new(dir_path);

    let ltm_results_path = dir_path.join("ltm_results.tsv");
    let expected = load_ltm_results(&ltm_results_path.to_string_lossy()).unwrap();

    ensure_ltm_results(&expected, &results, &loops);
}

#[test]
fn simulates_population_ltm() {
    simulate_ltm_path("../../test/logistic_growth_ltm/logistic_growth.stmx");
}

// --- Discovery mode integration tests ---

/// Run discovery mode on a model file and return discovered loops.
/// Simulation uses the VM path (compile_ltm_discovery_incremental);
/// Project::from is retained only for causal graph structural analysis.
fn discover_loops_from_path(model_path: &str) -> Vec<ltm_finding::FoundLoop> {
    let f = File::open(model_path).unwrap();
    let mut f = BufReader::new(f);
    let datamodel_project = xmile::project_from_reader(&mut f).unwrap();

    // VM discovery path for simulation
    let compiled = compile_ltm_discovery_incremental(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Project::from for causal graph structural analysis only
    let project = Project::from(datamodel_project);

    ltm_finding::discover_loops(&results, &project).expect("discover_loops should succeed")
}

#[test]
fn discovery_logistic_growth_finds_both_loops() {
    // The logistic growth model has exactly 2 loops:
    // 1. population -> births -> population (reinforcing)
    // 2. population -> fraction_used -> fractional_growth_rate -> births -> population (balancing)
    let found = discover_loops_from_path("../../test/logistic_growth_ltm/logistic_growth.stmx");

    assert_eq!(
        found.len(),
        2,
        "Discovery should find exactly 2 loops in logistic growth model, found {}",
        found.len()
    );

    // Verify the loops contain expected variables
    let has_population_births = found.iter().any(|l| {
        l.loop_info
            .links
            .iter()
            .any(|link| link.from.as_str() == "population" || link.to.as_str() == "population")
    });
    assert!(
        has_population_births,
        "Should find loops involving population"
    );
}

#[test]
fn discovery_cross_validates_with_exhaustive() {
    // Run both exhaustive and discovery mode on logistic growth model.
    // Discovery should find all loops that have significant contribution.
    let model_path = "../../test/logistic_growth_ltm/logistic_growth.stmx";

    // Exhaustive mode
    let f = File::open(model_path).unwrap();
    let mut f = BufReader::new(f);
    let datamodel_project = xmile::project_from_reader(&mut f).unwrap();
    let project = Project::from(datamodel_project);
    let main_ident: Ident<Canonical> = Ident::new("main");
    let exhaustive_loops = ltm::detect_loops(&project.models[&main_ident], &project).unwrap();

    let exhaustive_loop_count = exhaustive_loops.len();

    // Discovery mode
    let found = discover_loops_from_path(model_path);

    // Discovery should find all loops in a small model (only 2 loops, well under 1000)
    assert_eq!(
        found.len(),
        exhaustive_loop_count,
        "Discovery ({}) should find same number of loops as exhaustive ({}) for small models",
        found.len(),
        exhaustive_loop_count
    );

    // Verify that the discovered loops match the exhaustive loops by checking
    // that every exhaustive loop's node set appears in the discovery results
    for exhaustive_loop in &exhaustive_loops {
        let mut exhaustive_nodes: Vec<String> = exhaustive_loop
            .links
            .iter()
            .map(|l| l.from.as_str().to_string())
            .collect();
        exhaustive_nodes.sort();

        let found_match = found.iter().any(|f| {
            let mut found_nodes: Vec<String> = f
                .loop_info
                .links
                .iter()
                .map(|l| l.from.as_str().to_string())
                .collect();
            found_nodes.sort();
            found_nodes == exhaustive_nodes
        });

        assert!(
            found_match,
            "Exhaustive loop {} not found in discovery results",
            exhaustive_loop.format_path()
        );
    }
}

#[test]
fn discovery_arms_race_3party() {
    let model_path = "../../test/arms_race_3party/arms_race.stmx";

    // Exhaustive mode to establish ground truth
    let f = File::open(model_path).unwrap();
    let mut f = BufReader::new(f);
    let datamodel_project = xmile::project_from_reader(&mut f).unwrap();
    let project = Project::from(datamodel_project);
    let main_ident: Ident<Canonical> = Ident::new("main");
    let exhaustive_loops = ltm::detect_loops(&project.models[&main_ident], &project).unwrap();
    let exhaustive_count = exhaustive_loops.len();

    // The three-party arms race has 7 unique feedback loops: 3 self-adjustment
    // (balancing), 3 pairwise (reinforcing), and 1 three-way (reinforcing).
    // The second three-way loop (reverse direction) traverses the same node set
    // and is deduplicated by the exhaustive search.
    assert_eq!(
        exhaustive_count, 7,
        "Arms race should have 7 feedback loops, found {}",
        exhaustive_count
    );

    // Discovery mode
    let found = discover_loops_from_path(model_path);

    // With per-stock reset, discovery finds all 7 loops: each stock starts
    // with fresh best_scores, so pairwise and three-way reinforcing loops are
    // no longer pruned by scores from earlier stocks' self-loop searches.
    assert_eq!(
        found.len(),
        7,
        "Discovery should find all 7 loops in arms race model, found {}",
        found.len()
    );

    // All found loops should be a subset of the exhaustive results
    for found_loop in &found {
        let mut found_nodes: Vec<String> = found_loop
            .loop_info
            .links
            .iter()
            .map(|l| l.from.as_str().to_string())
            .collect();
        found_nodes.sort();

        let in_exhaustive = exhaustive_loops.iter().any(|exh| {
            let mut exh_nodes: Vec<String> = exh
                .links
                .iter()
                .map(|l| l.from.as_str().to_string())
                .collect();
            exh_nodes.sort();
            exh_nodes == found_nodes
        });
        assert!(
            in_exhaustive,
            "Discovered loop {} should exist in exhaustive results",
            found_loop.loop_info.format_path()
        );
    }
}

#[test]
fn discovery_decoupled_stocks() {
    let model_path = "../../test/decoupled_stocks/decoupled.stmx";

    // Cross-validate with exhaustive to establish ground truth
    let f = File::open(model_path).unwrap();
    let mut f = BufReader::new(f);
    let datamodel_project = xmile::project_from_reader(&mut f).unwrap();
    let project = Project::from(datamodel_project);
    let main_ident: Ident<Canonical> = Ident::new("main");
    let exhaustive_loops = ltm::detect_loops(&project.models[&main_ident], &project).unwrap();
    // Discovery mode -- the decoupled stocks model has time-varying loop
    // activity where different loops activate at different timesteps,
    // demonstrating why per-timestep discovery is necessary.
    let found = discover_loops_from_path(model_path);

    // The heuristic finds 2 of 3 loops: the self-loops for each stock.
    // The cross-stock loop is missed by the within-stock heuristic (the
    // strong self-loop paths set high best_scores on shared nodes during
    // each stock's own search, pruning the weaker cross-stock path).
    assert_eq!(
        found.len(),
        2,
        "Discovery should find 2 loops in decoupled model, found {}",
        found.len()
    );

    // All found loops should be a subset of the exhaustive results
    for found_loop in &found {
        let mut found_nodes: Vec<String> = found_loop
            .loop_info
            .links
            .iter()
            .map(|l| l.from.as_str().to_string())
            .collect();
        found_nodes.sort();

        let in_exhaustive = exhaustive_loops.iter().any(|exh| {
            let mut exh_nodes: Vec<String> = exh
                .links
                .iter()
                .map(|l| l.from.as_str().to_string())
                .collect();
            exh_nodes.sort();
            exh_nodes == found_nodes
        });
        assert!(
            in_exhaustive,
            "Discovered loop {} should exist in exhaustive results",
            found_loop.loop_info.format_path()
        );
    }
}

/// Checks for suspicious sign discontinuities in feedback loop relative score
/// time series.
///
/// A sign discontinuity is when consecutive data points flip from negative
/// to positive (or vice versa). When the magnitude barely changes across
/// the flip (e.g., -0.169 -> +0.169), it indicates a bug in the LTM sign
/// computation rather than genuinely changing loop behavior. A real polarity
/// transition would show the score approaching zero before crossing.
#[test]
fn hero_culture_loop_sign_continuity() {
    let f = File::open("../../test/hero_culture_ltm/hero_culture.sd.json").unwrap();
    let reader = BufReader::new(f);
    let json_project = json::Project::from_reader(reader).unwrap();
    let datamodel_project: simlin_engine::datamodel::Project = json_project.into();

    // VM path for simulation
    let compiled = compile_ltm_incremental(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Project::from for structural loop detection only
    let project = Project::from(datamodel_project);
    let main_ident: Ident<Canonical> = Ident::new("main");
    let loops = ltm::detect_loops(&project.models[&main_ident], &project).unwrap();
    assert!(
        !loops.is_empty(),
        "expected feedback loops from LTM analysis"
    );

    let mut failures: Vec<String> = Vec::new();

    for loop_item in &loops {
        let var_name = format!("$\u{205A}ltm\u{205A}rel_loop_score\u{205A}{}", loop_item.id);
        let var_ident =
            Ident::<Canonical>::from_str_unchecked(&Ident::new(&var_name).to_source_repr());

        let offset = match results.offsets.get(&var_ident) {
            Some(&off) => off,
            None => continue,
        };

        // Extract the time series for this loop
        let time_series: Vec<(f64, f64)> = results
            .iter()
            .enumerate()
            .map(|(step, row)| {
                let time = results.specs.start + results.specs.save_step * (step as f64);
                (time, row[offset])
            })
            .collect();

        if time_series.len() < 2 {
            continue;
        }

        // Find sign flips
        for i in 1..time_series.len() {
            let (_, prev_val) = time_series[i - 1];
            let (curr_t, curr_val) = time_series[i];

            if prev_val == 0.0 || curr_val == 0.0 {
                continue;
            }
            if prev_val.signum() == curr_val.signum() {
                continue;
            }

            // Sign flipped. Check if the magnitude barely changed --
            // that indicates a sign computation bug, not a genuine
            // polarity transition.
            let ratio = prev_val.abs() / curr_val.abs();
            if ratio > 0.5 && ratio < 2.0 {
                failures.push(format!(
                    "loop {} ({}): suspicious sign flip at t={:.0} where magnitude barely changes \
                     ({:.6} -> {:.6}, ratio={:.3}); \
                     this looks like a sign computation bug, not a genuine polarity transition",
                    loop_item.id,
                    loop_item.format_path(),
                    curr_t,
                    prev_val,
                    curr_val,
                    ratio,
                ));
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "Found {} suspicious sign discontinuities in loop scores:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}

// --- Module composite link score integration tests ---
//
// Tests involving stdlib modules (SMOOTH/DELAY) use the salsa/VM path
// (compile_project_incremental with ltm_enabled/ltm_discovery_mode).
//
// The layout resolution bug that caused "variable 'smth1' not found in layout
// during resolution" is fixed: LTM fragments whose SymVarRef names don't
// appear in the model's layout are now silently dropped during assembly
// (graceful degradation).  Most tests below are un-ignored; one remains
// #[ignore] because its failure has a different root cause:
//   - test_smooth_model_discovery_mode: discovery mode doesn't yet propagate
//     loop scores through SMOOTH composite paths

use simlin_engine::test_common::TestProject;

/// Regression: SMTH1 with an explicit initial_value argument (3rd arg) must
/// not cause LTM augmentation to reference a non-existent composite variable.
/// The initial_value port is only used for stock initialization and has no
/// runtime causal path to the output, so no composite is generated for it.
#[test]
fn test_smooth_with_initial_value_ltm() {
    let datamodel_project = TestProject::new("smooth_init_val")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "50", &["adj"], &[], None)
        .aux("init_val", "45", None)
        .aux("gap", "100 - level", None)
        .flow("adj", "SMTH1(gap, 5, init_val)", None)
        .build_datamodel();

    let compiled = compile_ltm_discovery_incremental(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("should simulate");
}

#[test]
fn test_smooth_goal_seeking_ltm() {
    // Goal-seeking model with SMOOTH in the feedback path:
    //   stock level = 50, inflow = adjustment
    //   adjustment = gap / adjustment_time
    //   gap = goal - SMTH1(level, smoothing_time)
    //   goal = 100, adjustment_time = 5, smoothing_time = 3
    use simlin_engine::db::model_detected_loops;

    let datamodel_project = TestProject::new("smooth_goal_ltm")
        .with_sim_time(0.0, 20.0, 0.25)
        .aux("goal", "100", None)
        .stock("level", "50", &["adjustment"], &[], None)
        .aux("smoothed_level", "SMTH1(level, 3)", None)
        .aux("gap", "goal - smoothed_level", None)
        .aux("adjustment_time", "5", None)
        .flow("adjustment", "gap / adjustment_time", None)
        .build_datamodel();

    // Structural analysis via salsa: verify loops are detected through
    // the SMOOTH module's composite causal path.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
    let source_model = sync.models["main"].source_model;
    let detected = model_detected_loops(&db, source_model, sync.project);
    assert!(
        !detected.loops.is_empty(),
        "Should detect at least one loop through SMOOTH"
    );

    // Simulation via VM with LTM enabled
    let compiled = compile_ltm_incremental(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("VM simulation should run");
    let results = vm.into_results();

    // Verify non-zero loop scores exist
    let loop_score_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| k.as_str().starts_with("$⁚ltm⁚loop_score⁚"))
        .collect();
    assert!(
        !loop_score_vars.is_empty(),
        "Should have loop score variables"
    );
}

// Still ignored: discovery mode doesn't yet find loops through SMOOTH composite paths.
#[test]
#[ignore]
fn test_smooth_model_discovery_mode() {
    // Same model as test_smooth_goal_seeking_ltm, but in discovery mode.
    let datamodel_project = TestProject::new("smooth_discovery")
        .with_sim_time(0.0, 20.0, 0.25)
        .aux("goal", "100", None)
        .stock("level", "50", &["adjustment"], &[], None)
        .aux("smoothed_level", "SMTH1(level, 3)", None)
        .aux("gap", "goal - smoothed_level", None)
        .aux("adjustment_time", "5", None)
        .flow("adjustment", "gap / adjustment_time", None)
        .build_datamodel();

    let compiled = compile_ltm_discovery_incremental(&datamodel_project);
    let project_dm = datamodel_project.clone();
    let project_for_discovery = Project::from(project_dm);
    let discovery_rc = std::sync::Arc::new(project_for_discovery);

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let found = ltm_finding::discover_loops(&results, &discovery_rc)
        .expect("discover_loops should succeed");

    assert!(
        !found.is_empty(),
        "Discovery mode should find loops through SMOOTH"
    );
}

#[test]
fn test_discovery_submodel_link_scores_excluded_from_search() {
    // Verify that sub-model link scores (interpunct-namespaced) are NOT
    // picked up by discovery mode's parse_link_offsets.
    //
    // With unified naming, sub-model link scores use the same
    // "$⁚ltm⁚link_score⁚" prefix but are namespaced by interpunct
    // (e.g., "module·$⁚ltm⁚link_score⁚..."). The discovery parser's
    // strip_prefix("$⁚ltm⁚link_score⁚") naturally excludes these
    // because interpunct-prefixed names don't start with that prefix.
    let datamodel_project = TestProject::new("submodel_link_exclusion")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "50", &["adj"], &[], None)
        .aux("gap", "100 - level", None)
        .flow("adj", "SMTH1(gap, 5)", None)
        .build_datamodel();

    let compiled = compile_ltm_discovery_incremental(&datamodel_project);
    let project_for_discovery = Project::from(datamodel_project.clone());
    let discovery_rc = std::sync::Arc::new(project_for_discovery);

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Root-level link scores should exist and start with the standard prefix
    let root_link_scores: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}")
        })
        .map(|k| k.as_str().to_string())
        .collect();
    assert!(
        !root_link_scores.is_empty(),
        "Should have root-level link score variables"
    );

    // Sub-model link scores (if present) should be namespaced with
    // interpunct and thus NOT start with the bare link_score prefix.
    let interpunct_link_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            let s = k.as_str();
            s.contains("\u{00B7}") && s.contains("link_score")
        })
        .map(|k| k.as_str().to_string())
        .collect();

    // Verify none of the interpunct-namespaced vars start with the root
    // prefix (which is what parse_link_offsets uses for discovery)
    for var in &interpunct_link_vars {
        assert!(
            !var.starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}"),
            "Interpunct-namespaced link score '{}' should not start with root prefix",
            var
        );
    }

    // Run discover_loops and verify only root-level links are found
    let found = ltm_finding::discover_loops(&results, &discovery_rc)
        .expect("discover_loops should succeed");

    // Discovered loops should only reference root-level variables (no interpunct)
    for loop_result in &found {
        for link in &loop_result.loop_info.links {
            assert!(
                !link.from.as_str().contains('\u{00B7}'),
                "Discovered link 'from' should not contain interpunct: {}",
                link.from.as_str()
            );
            assert!(
                !link.to.as_str().contains('\u{00B7}'),
                "Discovered link 'to' should not contain interpunct: {}",
                link.to.as_str()
            );
        }
    }
}

#[test]
fn test_multiple_smooth_instances() {
    // Two SMOOTH instances in different feedback paths.
    // Each should get its own internal composite scores.
    use simlin_engine::db::model_detected_loops;

    let datamodel_project = TestProject::new("multi_smooth")
        .with_sim_time(0.0, 10.0, 0.5)
        .stock("level_a", "50", &["adj_a"], &[], None)
        .aux("smoothed_a", "SMTH1(level_a, 3)", None)
        .aux("gap_a", "100 - smoothed_a", None)
        .flow("adj_a", "gap_a / 5", None)
        .stock("level_b", "30", &["adj_b"], &[], None)
        .aux("smoothed_b", "SMTH1(level_b, 2)", None)
        .aux("gap_b", "80 - smoothed_b", None)
        .flow("adj_b", "gap_b / 3", None)
        .build_datamodel();

    // Structural analysis via salsa: each stock-flow path through a
    // SMOOTH creates a feedback loop.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
    let source_model = sync.models["main"].source_model;
    let detected = model_detected_loops(&db, source_model, sync.project);
    assert!(
        detected.loops.len() >= 2,
        "Should detect at least 2 loops (one per SMOOTH feedback path), found {}",
        detected.loops.len()
    );

    // Simulation via VM with LTM enabled
    let compiled = compile_ltm_incremental(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("should simulate");
    let results = vm.into_results();

    // Verify we have loop score variables for each independent loop
    let loop_scores: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| k.as_str().starts_with("$⁚ltm⁚loop_score⁚"))
        .collect();
    assert!(
        loop_scores.len() >= 2,
        "Should have at least 2 loop score variables, found {}",
        loop_scores.len()
    );
}

#[test]
fn test_internal_smooth_loop_not_in_parent() {
    // The smth1 module has an internal balancing loop (output -> flow -> output).
    // This should NOT appear in the parent model's loop list.
    //
    // Uses salsa-based model_detected_loops for structural analysis (no
    // simulation needed). This is the preferred path for structural loop
    // queries.
    use simlin_engine::db::model_detected_loops;

    let datamodel_project = TestProject::new("internal_loop_suppression")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "50", &["adj"], &[], None)
        .aux("gap", "100 - level", None)
        .flow("adj", "SMTH1(gap, 5)", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
    let source_model = sync.models["main"].source_model;
    let detected = model_detected_loops(&db, source_model, sync.project);

    // No loop should contain only internal module variables.
    // Parent loops should involve parent-level variables like "level",
    // "gap", "adj", not just stdlib internals like "flow", "output".
    let internal_names: std::collections::HashSet<&str> =
        ["flow", "output"].iter().copied().collect();
    for loop_item in &detected.loops {
        let all_internal = loop_item
            .variables
            .iter()
            .all(|v| internal_names.contains(v.as_str()));
        assert!(
            !all_internal,
            "Parent loops should not be purely internal module loops. Loop {:?} has vars: {:?}",
            loop_item.id, loop_item.variables
        );
    }
}

// --- Cycle partition integration tests ---

#[test]
fn test_independent_subsystems_partitioned_relative_scores() {
    // Two completely independent stock-flow loops:
    // Subsystem 1 (balancing): stock_a (init=50), gap_a = 100 - stock_a, flow_a = gap_a / 5
    // Subsystem 2 (reinforcing): stock_b (init=10), flow_b = stock_b * 0.1
    //
    // Each loop's relative score should be +/-1.0 for all non-zero timesteps,
    // because each loop is the ONLY loop in its partition.
    let datamodel_project = TestProject::new("indep_subsystems")
        .with_sim_time(0.0, 10.0, 0.25)
        .stock("stock_a", "50", &["flow_a"], &[], None)
        .aux("gap_a", "100 - stock_a", None)
        .flow("flow_a", "gap_a / 5", None)
        .stock("stock_b", "10", &["flow_b"], &[], None)
        .flow("flow_b", "stock_b * 0.1", None)
        .build_datamodel();

    let compiled = compile_ltm_incremental(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Find relative loop score variables
    let rel_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| k.as_str().starts_with("$⁚ltm⁚rel_loop_score⁚"))
        .cloned()
        .collect();

    assert_eq!(
        rel_vars.len(),
        2,
        "Should have exactly 2 relative loop score variables, found {}",
        rel_vars.len()
    );

    // Each loop is alone in its partition, so each relative score should be +/-1.0
    for var in &rel_vars {
        let offset = results.offsets[var];
        let scores: Vec<f64> = (0..results.step_count)
            .map(|step| results.data[step * results.step_size + offset])
            .collect();

        let nonzero_scores: Vec<f64> = scores
            .iter()
            .copied()
            .filter(|v| *v != 0.0 && !v.is_nan())
            .collect();

        assert!(
            !nonzero_scores.is_empty(),
            "Should have non-zero relative scores for {}",
            var.as_str()
        );

        for score in &nonzero_scores {
            assert!(
                (score.abs() - 1.0).abs() < 1e-6,
                "Single-loop-per-partition relative score should have |value| = 1, got {} for {}",
                score,
                var.as_str()
            );
        }
    }
}

#[test]
fn test_coupled_two_stock_single_partition() {
    // Predator-prey: both stocks mutually reachable through flows
    let datamodel_project = TestProject::new("coupled_pred_prey")
        .with_sim_time(0.0, 20.0, 0.25)
        .stock("prey", "100", &["prey_births"], &["prey_deaths"], None)
        .flow("prey_births", "prey * 0.1", None)
        .flow("prey_deaths", "prey * predators * 0.01", None)
        .stock("predators", "10", &["pred_births"], &["pred_deaths"], None)
        .flow("pred_births", "predators * prey * 0.001", None)
        .flow("pred_deaths", "predators * 0.05", None)
        .build_datamodel();

    // Structural partition analysis via Project::from
    let project = Project::from(datamodel_project.clone());
    let main_ident: Ident<Canonical> = Ident::new("main");
    let graph = ltm::CausalGraph::from_model(&project.models[&main_ident], &project).unwrap();
    let partitions = graph.compute_cycle_partitions();

    // Both stocks should be in the same partition
    assert_eq!(
        partitions.partitions.len(),
        1,
        "Mutually-reachable stocks should be in one partition, got {}",
        partitions.partitions.len()
    );
    assert_eq!(partitions.partitions[0].len(), 2);

    // VM path for LTM simulation
    let compiled = compile_ltm_incremental(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Verify relative loop scores exist
    let rel_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| k.as_str().starts_with("$⁚ltm⁚rel_loop_score⁚"))
        .collect();
    assert!(
        !rel_vars.is_empty(),
        "Should have relative loop score variables"
    );
}

#[test]
fn test_discovery_independent_subsystems() {
    // Same two independent subsystems, but using discovery mode.
    // Both subsystem loops should be retained.
    let datamodel_project = TestProject::new("indep_discovery")
        .with_sim_time(0.0, 10.0, 0.25)
        .stock("stock_a", "50", &["flow_a"], &[], None)
        .aux("gap_a", "100 - stock_a", None)
        .flow("flow_a", "gap_a / 5", None)
        .stock("stock_b", "10", &["flow_b"], &[], None)
        .flow("flow_b", "stock_b * 0.1", None)
        .build_datamodel();

    // VM discovery path for simulation
    let compiled = compile_ltm_discovery_incremental(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Project::from for causal graph structural analysis only
    let project = Project::from(datamodel_project);

    let found =
        ltm_finding::discover_loops(&results, &project).expect("discover_loops should succeed");

    assert!(
        found.len() >= 2,
        "Discovery should find at least 2 loops (one per subsystem), found {}",
        found.len()
    );
}

#[test]
fn test_arms_race_single_partition() {
    use simlin_engine::db::{SimlinDb, model_cycle_partitions, sync_from_datamodel_incremental};
    use std::fs::File;
    use std::io::BufReader;

    let f = File::open("../../test/arms_race_3party/arms_race.stmx").unwrap();
    let mut f = BufReader::new(f);
    let datamodel_project = xmile::project_from_reader(&mut f).unwrap();

    // Use salsa-based cycle partition analysis
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
    let source_model = sync.models["main"].source_model;
    let partitions = model_cycle_partitions(&db, source_model, sync.project);

    // All 3 stocks should be in a single partition (mutually reachable)
    assert_eq!(
        partitions.partitions.len(),
        1,
        "Arms race stocks should all be in one partition, got {}",
        partitions.partitions.len()
    );
    assert_eq!(
        partitions.partitions[0].len(),
        3,
        "Should have 3 stocks in the partition"
    );
}

// Ignored: the causal edge name for implicit module instances
// (e.g., "$:combined:0:smth1") does not match the identifier used in the
// downstream variable's equation AST. The ceteris-paribus analysis cannot
// isolate the module's contribution because it cannot find the from_ident
// in the dependency set, so it wraps all deps with PREVIOUS and produces
// magnitude ~1. Fixing this requires the causal graph to use the same
// variable names as the equation AST.
#[test]
#[ignore]
fn test_module_output_multi_input_link_score_magnitude() {
    // When a module output shares a downstream equation with another input,
    // the link score for module -> downstream should NOT always be magnitude 1.
    // It should reflect the partial contribution of the module output.
    //
    // Model: level (stock) -> adjustment (flow) -> level
    //        combined = SMTH1(level, 3) * 0.5 + other_input * 0.5
    //        other_input = TIME * 3
    //        adjustment = 100 - combined
    //
    // The SMTH1 output and other_input both contribute ~50% to combined.
    //
    let datamodel_project = TestProject::new("module_multi_input")
        .with_sim_time(0.0, 20.0, 0.25)
        .stock("level", "50", &["adjustment"], &[], None)
        .aux("other_input", "TIME * 3", None)
        .aux(
            "combined",
            "SMTH1(level, 3) * 0.5 + other_input * 0.5",
            None,
        )
        .flow("adjustment", "100 - combined", None)
        .build_datamodel();

    let compiled = compile_ltm_discovery_incremental(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("simulation should run");
    let results = vm.into_results();

    // Find the link score for the smth1 module -> combined
    let module_link_offset = results
        .offsets
        .iter()
        .find(|(k, _)| {
            let s = k.as_str();
            s.starts_with("$⁚ltm⁚link_score⁚") && s.contains("smth1") && s.ends_with("→combined")
        })
        .map(|(_, &offset)| offset);

    let offset =
        module_link_offset.expect("should have a link score variable for module -> combined");

    // Check link scores after the initial settling period (skip t=0 and first
    // few steps where PREVIOUS values are not yet populated).
    let mut found_non_unity = false;
    for step in 8..results.step_count {
        let value = results.data[step * results.step_size + offset];
        if value.is_nan() || value == 0.0 {
            continue;
        }
        let magnitude = value.abs();
        if magnitude < 0.95 {
            found_non_unity = true;
            break;
        }
    }

    assert!(
        found_non_unity,
        "module -> combined link score magnitude should be significantly less than 1 \
         when the downstream variable has multiple inputs contributing. \
         All observed magnitudes were >= 0.95, indicating the black-box formula is \
         still being used."
    );
}

// --- VM integration tests for LTM scoring ---
//
// These tests exercise the salsa/VM path (compile_ltm_incremental and
// compile_ltm_discovery_incremental) for models without stdlib modules.

/// Balancing feedback loop (stock -> aux -> flow -> stock) via the full
/// VM pipeline in exhaustive mode. Verifies non-zero link scores and
/// loop/relative loop scores after simulation.
#[test]
fn test_feedback_loop_exhaustive_vm() {
    let project = TestProject::new("fb_exhaustive")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "50", &["adj"], &[], None)
        .aux("gap", "100 - level", None)
        .flow("adj", "gap / 5", None)
        .build_datamodel();

    let compiled = compile_ltm_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let link_score_offsets: Vec<_> = results
        .offsets
        .iter()
        .filter(|(name, _)| name.as_str().contains("link_score"))
        .collect();
    assert!(
        !link_score_offsets.is_empty(),
        "should have link score variables"
    );

    // Exhaustive mode should produce loop score and relative loop score vars
    let loop_score_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}")
        })
        .collect();
    assert!(
        !loop_score_vars.is_empty(),
        "exhaustive mode should have loop score variables"
    );

    let rel_score_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}rel_loop_score\u{205A}")
        })
        .collect();
    assert!(
        !rel_score_vars.is_empty(),
        "exhaustive mode should have relative loop score variables"
    );

    let any_nonzero_link = link_score_offsets.iter().any(|(_, offset)| {
        (2..results.step_count)
            .any(|step| results.data[step * results.step_size + **offset].abs() > 1e-10)
    });
    assert!(
        any_nonzero_link,
        "at least one link score should be non-zero"
    );
}

/// Feedback loop via the VM pipeline in discovery mode. Verifies that
/// discovery mode produces link scores for all causal edges and that
/// discover_loops finds the feedback loop.
#[test]
fn test_feedback_loop_discovery_vm() {
    let project = TestProject::new("fb_discovery")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "50", &["adj"], &[], None)
        .aux("gap", "100 - level", None)
        .flow("adj", "gap / 5", None)
        .build_datamodel();

    let compiled = compile_ltm_discovery_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let link_score_offsets: Vec<_> = results
        .offsets
        .iter()
        .filter(|(name, _)| {
            name.as_str()
                .starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}")
        })
        .collect();
    assert!(
        !link_score_offsets.is_empty(),
        "discovery mode should have link score variables"
    );

    // Use the interpreter Project path to run discover_loops on the VM results,
    // since discover_loops needs a compiled Project for the causal graph.
    let compiled_project = Project::from(project);
    let found = ltm_finding::discover_loops(&results, &compiled_project)
        .expect("discover_loops should succeed");

    assert!(
        !found.is_empty(),
        "discovery mode should find at least one loop"
    );

    // The discovered loop should involve the stock
    let involves_level = found.iter().any(|l| {
        l.loop_info
            .links
            .iter()
            .any(|link| link.from.as_str() == "level" || link.to.as_str() == "level")
    });
    assert!(
        involves_level,
        "discovered loop should involve the stock variable"
    );
}

/// Reinforcing feedback loop with DELAY1-like dynamics (no actual stdlib
/// module) via the VM pipeline. Uses a stock-flow structure with the flow
/// depending on the stock to create a reinforcing loop, verifying that LTM
/// scoring works for reinforcing as well as balancing loops.
#[test]
fn test_reinforcing_feedback_loop_vm() {
    let project = TestProject::new("reinforcing_fb")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "100", &["births"], &[], None)
        .aux("growth_rate", "0.1", None)
        .flow("births", "population * growth_rate", None)
        .build_datamodel();

    let compiled = compile_ltm_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let link_score_offsets: Vec<_> = results
        .offsets
        .iter()
        .filter(|(name, _)| name.as_str().contains("link_score"))
        .collect();
    assert!(
        !link_score_offsets.is_empty(),
        "reinforcing loop should have link score variables"
    );

    let any_nonzero_link = link_score_offsets.iter().any(|(_, offset)| {
        (2..results.step_count)
            .any(|step| results.data[step * results.step_size + **offset].abs() > 1e-10)
    });
    assert!(
        any_nonzero_link,
        "at least one reinforcing loop link score should be non-zero"
    );
}

/// Two independent feedback loops via the VM pipeline. Each loop should
/// produce its own link scores and loop scores, verifying independent
/// scoring per feedback path.
#[test]
fn test_multiple_feedback_loops_vm() {
    let project = TestProject::new("multi_loops_vm")
        .with_sim_time(0.0, 10.0, 0.5)
        .stock("level_a", "50", &["adj_a"], &[], None)
        .aux("gap_a", "100 - level_a", None)
        .flow("adj_a", "gap_a / 5", None)
        .stock("level_b", "30", &["adj_b"], &[], None)
        .aux("gap_b", "80 - level_b", None)
        .flow("adj_b", "gap_b / 3", None)
        .build_datamodel();

    let compiled = compile_ltm_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Both loops should produce link score variables
    let link_score_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}")
        })
        .cloned()
        .collect();

    // Each loop has at least 3 edges (stock->aux, aux->flow, flow->stock)
    assert!(
        link_score_vars.len() >= 6,
        "two loops should have at least 6 link score variables, found {}",
        link_score_vars.len()
    );

    // Both loops should produce independent loop scores
    let loop_score_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}")
        })
        .collect();
    assert!(
        loop_score_vars.len() >= 2,
        "should have at least 2 loop score variables, found {}",
        loop_score_vars.len()
    );

    // Relative loop scores should all have magnitude 1 since each loop
    // is in its own partition (independent subsystems).
    let rel_score_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}rel_loop_score\u{205A}")
        })
        .collect();
    assert_eq!(
        rel_score_vars.len(),
        loop_score_vars.len(),
        "should have a relative loop score for each loop score"
    );
}

/// A model with no modules (passthrough aux in feedback loop). LTM
/// compilation should succeed with no composite or pathway scores since
/// there are no module instances.
#[test]
fn test_passthrough_module_ltm_vm() {
    let project = TestProject::new("passthrough_vm")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "50", &["inflow"], &[], None)
        .aux("passthrough", "100 - level", None)
        .flow("inflow", "passthrough / 5", None)
        .build_datamodel();

    let compiled = compile_ltm_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // No composite scores should exist since there are no modules
    let composite_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| k.as_str().contains("composite"))
        .collect();
    assert!(
        composite_vars.is_empty(),
        "passthrough model without modules should have no composite scores, found: {:?}",
        composite_vars
            .iter()
            .map(|k| k.as_str())
            .collect::<Vec<_>>()
    );

    // But link scores should still exist for the non-module causal edges
    let link_score_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}")
        })
        .collect();
    assert!(
        !link_score_vars.is_empty(),
        "should still have link scores for non-module edges"
    );
}

/// User-defined module with an internal stock, wired into a parent-level
/// feedback loop. Verifies that LTM scoring works end-to-end through the
/// compile_project_incremental + VM pipeline for user-defined modules.
///
/// Model structure:
///   Parent: level -> gap -> [growth_model] -> adjustment -> level
///   Sub-model "growth": input_signal -> growth_flow -> internal_level -> output
///
/// The parent feeds `gap` to the sub-model's `input_signal`, and uses
/// `growth_model.output` in the adjustment flow. The sub-model's output is
/// named `output` (not `output_rate`) so the LTM pathway analyzer can find
/// the causal path from `input_signal` to `output` and generate a composite
/// score for the input port.
#[test]
fn test_user_defined_module_ltm_vm() {
    use simlin_engine::datamodel;

    let project = datamodel::Project {
        name: "user_module_ltm".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Stock(datamodel::Stock {
                        ident: "level".to_string(),
                        equation: datamodel::Equation::Scalar("50".to_string()),
                        documentation: String::new(),
                        units: None,
                        inflows: vec!["adjustment".to_string()],
                        outflows: vec![],
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "gap".to_string(),
                        equation: datamodel::Equation::Scalar("100 - level".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "growth_model".to_string(),
                        model_name: "growth".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "gap".to_string(),
                            dst: "growth_model.input_signal".to_string(),
                        }],
                        compat: datamodel::Compat::default(),
                        ai_state: None,
                        uid: None,
                    }),
                    datamodel::Variable::Flow(datamodel::Flow {
                        ident: "adjustment".to_string(),
                        equation: datamodel::Equation::Scalar("growth_model.output".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            },
            datamodel::Model {
                name: "growth".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "input_signal".to_string(),
                        equation: datamodel::Equation::Scalar("0".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat {
                            can_be_module_input: true,
                            ..datamodel::Compat::default()
                        },
                    }),
                    datamodel::Variable::Stock(datamodel::Stock {
                        ident: "internal_level".to_string(),
                        equation: datamodel::Equation::Scalar("0".to_string()),
                        documentation: String::new(),
                        units: None,
                        inflows: vec!["growth_flow".to_string()],
                        outflows: vec![],
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Flow(datamodel::Flow {
                        ident: "growth_flow".to_string(),
                        equation: datamodel::Equation::Scalar("input_signal / 5".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    // Named "output" (not "output_rate") so the LTM pathway analyzer
                    // can find the causal path from input_signal to output and
                    // generate a composite score for the input_signal port.
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "output".to_string(),
                        equation: datamodel::Equation::Scalar("internal_level * 0.1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            },
        ],
        source: None,
        ai_information: None,
    };

    let compiled = compile_ltm_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Verify link scores exist for the parent model's feedback loop
    let link_score_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}")
        })
        .collect();
    assert!(
        !link_score_vars.is_empty(),
        "should have link score variables for parent feedback loop"
    );

    // Verify at least one link score is non-zero after initial timesteps
    let any_nonzero_link = link_score_vars.iter().any(|k| {
        let offset = results.offsets[*k];
        (2..results.step_count)
            .any(|step| results.data[step * results.step_size + offset].abs() > 1e-10)
    });
    assert!(
        any_nonzero_link,
        "at least one link score should be non-zero in user-defined module model"
    );

    // Verify the sub-model's internal variables are present in results
    // with the module instance prefix (growth_model.varname)
    let submodel_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| k.as_str().starts_with("growth_model."))
        .collect();
    assert!(
        !submodel_vars.is_empty(),
        "sub-model variables should be present with module prefix in results, \
         available keys: {:?}",
        results
            .offsets
            .keys()
            .map(|k| k.as_str().to_string())
            .collect::<Vec<_>>()
    );

    // Verify the sub-model's internal stock is accessible
    let has_internal_stock = submodel_vars
        .iter()
        .any(|k| k.as_str() == "growth_model.internal_level");
    assert!(
        has_internal_stock,
        "should be able to access the sub-model's internal stock via qualified name"
    );

    // Verify the module appears as a node in the causal graph link scores.
    // User-defined modules with internal stocks participate as causal nodes.
    let has_module_link = link_score_vars.iter().any(|k| {
        let s = k.as_str();
        s.contains("growth_model")
    });
    assert!(
        has_module_link,
        "link scores should reference the user-defined module as a causal node"
    );

    // Verify composite score variables exist for the sub-model's input port.
    // The "growth" sub-model has "input_signal" as its input port and "output"
    // as its output. The LTM pathway analyzer finds the causal path
    // input_signal -> growth_flow -> internal_level -> output and generates
    // a composite score for the input_signal port. In the parent's results the
    // composite is namespaced by the module instance name (growth_model.).
    let composite_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            let s = k.as_str();
            s.starts_with("growth_model.") && s.contains("composite")
        })
        .collect();
    assert!(
        !composite_vars.is_empty(),
        "sub-model composite score variables should exist namespaced by the module \
         instance name (growth_model.*composite*), available keys: {:?}",
        results
            .offsets
            .keys()
            .filter(|k| k.as_str().starts_with("growth_model."))
            .map(|k| k.as_str().to_string())
            .collect::<Vec<_>>()
    );

    // Verify composite for the input_signal port specifically
    let has_input_signal_composite = composite_vars.iter().any(|k| {
        let s = k.as_str();
        s.contains("input_signal")
    });
    assert!(
        has_input_signal_composite,
        "composite score for the input_signal port should exist in results, \
         found composite vars: {:?}",
        composite_vars
            .iter()
            .map(|k| k.as_str().to_string())
            .collect::<Vec<_>>()
    );

    // Verify loop and relative loop scores exist (exhaustive mode)
    let loop_score_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}")
        })
        .collect();
    assert!(
        !loop_score_vars.is_empty(),
        "exhaustive mode should produce loop scores for model with user-defined module"
    );

    let rel_score_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}rel_loop_score\u{205A}")
        })
        .collect();
    assert!(
        !rel_score_vars.is_empty(),
        "exhaustive mode should produce relative loop scores"
    );
}

/// Nested module: a user-defined sub-model that internally uses SMOOTH,
/// creating two levels of module nesting (root -> user module -> stdlib
/// SMOOTH module). Verifies LTM scoring at both nesting levels.
///
/// Model structure:
///   Parent: level -> gap -> [processor] -> adjustment -> level
///   Sub-model "processor": input -> smoothed (SMTH1) -> output
#[test]
fn test_nested_module_ltm_vm() {
    use simlin_engine::datamodel;

    let project = datamodel::Project {
        name: "nested_module_ltm".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 20.0,
            dt: datamodel::Dt::Dt(0.25),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Stock(datamodel::Stock {
                        ident: "level".to_string(),
                        equation: datamodel::Equation::Scalar("50".to_string()),
                        documentation: String::new(),
                        units: None,
                        inflows: vec!["adjustment".to_string()],
                        outflows: vec![],
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "gap".to_string(),
                        equation: datamodel::Equation::Scalar("100 - level".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "processor".to_string(),
                        model_name: "processor".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "gap".to_string(),
                            dst: "processor.input".to_string(),
                        }],
                        compat: datamodel::Compat::default(),
                        ai_state: None,
                        uid: None,
                    }),
                    datamodel::Variable::Flow(datamodel::Flow {
                        ident: "adjustment".to_string(),
                        equation: datamodel::Equation::Scalar("processor.output / 5".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            },
            datamodel::Model {
                name: "processor".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "input".to_string(),
                        equation: datamodel::Equation::Scalar("0".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat {
                            can_be_module_input: true,
                            ..datamodel::Compat::default()
                        },
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "smoothed".to_string(),
                        equation: datamodel::Equation::Scalar("SMTH1(input, 3)".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "output".to_string(),
                        equation: datamodel::Equation::Scalar("smoothed".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            },
        ],
        source: None,
        ai_information: None,
    };

    // Compile and simulate with LTM via the salsa/VM path.
    let compiled = compile_ltm_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("simulation should run");
    let results = vm.into_results();

    // Verify link scores exist at the parent level
    let link_score_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}")
        })
        .collect();
    assert!(
        !link_score_vars.is_empty(),
        "should have link score variables for the parent feedback loop"
    );

    // Verify at least one link score is non-zero after initial timesteps
    let any_nonzero_link = link_score_vars.iter().any(|k| {
        let offset = results.offsets[*k];
        (8..results.step_count)
            .any(|step| results.data[step * results.step_size + offset].abs() > 1e-10)
    });
    assert!(
        any_nonzero_link,
        "at least one link score should be non-zero in nested module model"
    );

    // Verify loop scores exist (exhaustive mode)
    let loop_score_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}")
        })
        .collect();
    assert!(
        !loop_score_vars.is_empty(),
        "exhaustive mode should produce loop scores for nested module model"
    );

    // Verify relative loop scores exist
    let rel_score_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}rel_loop_score\u{205A}")
        })
        .collect();
    assert!(
        !rel_score_vars.is_empty(),
        "exhaustive mode should produce relative loop scores for nested module model"
    );

    // Verify the processor module appears in the causal link scores,
    // confirming that the user-defined module (which internally uses
    // SMOOTH) is treated as a causal node in the parent model's
    // feedback loop.
    let has_processor_link = link_score_vars
        .iter()
        .any(|k| k.as_str().contains("processor"));
    assert!(
        has_processor_link,
        "link scores should reference the 'processor' user module as a causal node, \
         available link scores: {:?}",
        link_score_vars
            .iter()
            .map(|k| k.as_str())
            .collect::<Vec<_>>()
    );

    // AC4.2: Composite scores for the user module's nested SMOOTH instance
    // exist and are namespaced under the module instance name.
    //
    // The "processor" model uses SMTH1 internally. The stdlib SMOOTH sub-model
    // has input ports ("input", "delay_time") and an internal stock, so the LTM
    // pipeline generates composite scores for those ports. These composites appear
    // in results namespaced by the full chain: processor.<smth1_instance>.
    let nested_composite_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            let s = k.as_str();
            s.starts_with("processor.") && s.contains("composite")
        })
        .collect();
    assert!(
        !nested_composite_vars.is_empty(),
        "composite scores should exist for the SMOOTH instance nested inside 'processor', \
         namespaced under processor.*, available processor.* keys: {:?}",
        results
            .offsets
            .keys()
            .filter(|k| k.as_str().starts_with("processor."))
            .map(|k| k.as_str().to_string())
            .collect::<Vec<_>>()
    );

    // AC4.3: Chained nesting notation is present in composite score keys.
    // The composite vars for the SMOOTH inside processor use multi-segment
    // keys like "processor.$⁚smoothed⁚0⁚smth1.$⁚ltm⁚composite⁚input",
    // confirming that nested module namespacing works correctly.
    let has_chained_composite = nested_composite_vars.iter().any(|k| {
        let s = k.as_str();
        // More than one "$" in the key means multiple levels of nesting are encoded
        s.matches('$').count() >= 2
    });
    assert!(
        has_chained_composite,
        "composite keys should reflect chained nesting (multiple '$' segments), \
         found composite vars: {:?}",
        nested_composite_vars
            .iter()
            .map(|k| k.as_str().to_string())
            .collect::<Vec<_>>()
    );

    // AC4.3: Non-zero link scores exist at the nested (SMOOTH-internal) level.
    // The SMOOTH sub-model has link scores for its own causal edges (e.g.,
    // input→flow, flow→output). These should be non-zero after the initial steps.
    let nested_link_score_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            let s = k.as_str();
            s.starts_with("processor.") && s.contains("link_score") && !s.contains("arg0")
        })
        .collect();
    assert!(
        !nested_link_score_vars.is_empty(),
        "link score variables should exist inside the SMOOTH nested in 'processor'"
    );

    let any_nonzero_nested_link = nested_link_score_vars.iter().any(|k| {
        let offset = results.offsets[*k];
        (8..results.step_count)
            .any(|step| results.data[step * results.step_size + offset].abs() > 1e-10)
    });
    assert!(
        any_nonzero_nested_link,
        "at least one nested link score (inside processor's SMOOTH) should be non-zero, \
         nested link score vars: {:?}",
        nested_link_score_vars
            .iter()
            .map(|k| k.as_str().to_string())
            .collect::<Vec<_>>()
    );
}
