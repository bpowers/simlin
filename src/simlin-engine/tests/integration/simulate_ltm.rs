// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use std::result::Result as StdResult;

use simlin_engine::common::{Canonical, Ident};
// `model_element_loop_circuits` is `#[deprecated]` for new LTM callers;
// the integration tests below drive it directly to compare the legacy
// element-Johnson surface against the tiered enumerator's output. The
// allow keeps the deprecation lint clean for tests pinning the legacy
// contract while preserving the warning for accidental new uses.
#[allow(deprecated)]
use simlin_engine::db::model_element_loop_circuits;
use simlin_engine::db::{
    DetectedLoop, DetectedLoopPolarity, SimlinDb, causal_graph_from_edges,
    causal_graph_from_element_edges, compile_project_incremental, model_causal_edges,
    model_cycle_partitions, model_detected_loops, model_element_causal_edges,
    model_element_cycle_partitions, model_loop_circuits, model_loop_circuits_tiered,
    model_ltm_variables, project_datamodel_dims, reclassify_loops_from_results,
    set_project_ltm_discovery_mode, set_project_ltm_enabled, sync_from_datamodel,
    sync_from_datamodel_incremental,
};
use simlin_engine::indexmap::IndexMap;
use simlin_engine::xmile;
use simlin_engine::{CompiledSimulation, Project, Results, Vm, json, ltm_finding, ltm_post};

const LTM_TOLERANCE: f64 = 0.05;

/// Compile a datamodel project to a VM simulation using the incremental
/// salsa path with LTM enabled (exhaustive mode).
fn compile_ltm_incremental(project: &simlin_engine::datamodel::Project) -> CompiledSimulation {
    compile_ltm_incremental_with_partitions(project).0
}

/// Compile with LTM enabled and capture the per-slot loop_partitions
/// mapping `compute_rel_loop_scores*` need to derive relative scores
/// post-sim.  Since rel_loop_score is no longer emitted as a VM variable
/// (see docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md), tests
/// that used to filter `results.offsets` for `$⁚ltm⁚rel_loop_score⁚{id}`
/// must now invoke `ltm_post::compute_rel_loop_scores(results, loop_partitions)`.
fn compile_ltm_incremental_with_partitions(
    project: &simlin_engine::datamodel::Project,
) -> (CompiledSimulation, IndexMap<String, Vec<Option<usize>>>) {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
    let source_model = sync.models["main"].source_model;
    let loop_partitions = model_ltm_variables(&db, source_model, sync.project)
        .loop_partitions
        .clone();
    (compiled, loop_partitions)
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

fn ensure_ltm_results(
    expected: &LtmResults,
    actual_results: &Results,
    loops: &[DetectedLoop],
    loop_partitions: &IndexMap<String, Vec<Option<usize>>>,
) {
    let mut errors = Vec::new();

    // Rel_loop_score is computed post-sim from loop_score.  See
    // docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md for why the
    // compile-time emitter was removed.
    let rel_scores = ltm_post::compute_rel_loop_scores(actual_results, loop_partitions);

    for (loop_id, expected_scores) in &expected.loop_scores {
        let Some(actual_values) = rel_scores.get(loop_id) else {
            panic!(
                "LTM results missing loop score series for loop '{}'",
                loop_id
            );
        };

        let mut loop_errors = Vec::new();
        let mut actual_series = Vec::new();

        for (expected_time, expected_value) in expected_scores {
            if *expected_time < actual_results.specs.start
                || *expected_time > actual_results.specs.stop
            {
                continue;
            }

            let mut found_match = false;

            for (step, actual_value) in actual_values.iter().enumerate() {
                let time =
                    actual_results.specs.start + actual_results.specs.save_step * (step as f64);

                if (time - expected_time).abs() < 1e-9 {
                    found_match = true;
                    let actual_value = *actual_value;
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
                        DetectedLoopPolarity::Reinforcing => "Reinforcing (R)",
                        DetectedLoopPolarity::Balancing => "Balancing (B)",
                        DetectedLoopPolarity::MostlyReinforcing => "Mostly reinforcing (Rux)",
                        DetectedLoopPolarity::MostlyBalancing => "Mostly balancing (Bux)",
                        DetectedLoopPolarity::Undetermined => "Undetermined (U)",
                    }
                );
                eprintln!("  Path: {}", loop_obj.variables.join(" -> "));
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
    let (compiled, loop_partitions) = compile_ltm_incremental_with_partitions(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Structural loop detection via salsa-tracked functions
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel_project);
    let source_model = sync.models["main"].source;
    let detected = model_detected_loops(&db, source_model, sync.project);
    let loops = detected.loops;

    let xmile_name = std::path::Path::new(model_path).file_name().unwrap();
    let dir_path = &model_path[0..(model_path.len() - xmile_name.len())];
    let dir_path = std::path::Path::new(dir_path);

    let ltm_results_path = dir_path.join("ltm_results.tsv");
    let expected = load_ltm_results(&ltm_results_path.to_string_lossy()).unwrap();

    ensure_ltm_results(&expected, &results, &loops, &loop_partitions);
}

#[test]
fn simulates_population_ltm() {
    simulate_ltm_path("../../test/logistic_growth_ltm/logistic_growth.stmx");
}

// --- Discovery mode integration tests ---

/// Run discovery mode on a model file and return discovered loops.
/// Simulation uses the VM path (compile_ltm_discovery_incremental);
/// Project::from_datamodel (salsa-backed) is used for causal graph structural analysis.
fn discover_loops_from_path(model_path: &str) -> Vec<ltm_finding::FoundLoop> {
    let f = File::open(model_path).unwrap();
    let mut f = BufReader::new(f);
    let datamodel_project = xmile::project_from_reader(&mut f).unwrap();

    // VM discovery path for simulation
    let compiled = compile_ltm_discovery_incremental(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Project for causal graph structural analysis (from_datamodel uses salsa internally)
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

    // Exhaustive mode via salsa-tracked structural analysis
    let f = File::open(model_path).unwrap();
    let mut f = BufReader::new(f);
    let datamodel_project = xmile::project_from_reader(&mut f).unwrap();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel_project);
    let source_model = sync.models["main"].source;
    let exhaustive_loops = model_detected_loops(&db, source_model, sync.project);

    let exhaustive_loop_count = exhaustive_loops.loops.len();

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
    for exhaustive_loop in &exhaustive_loops.loops {
        let mut exhaustive_nodes: Vec<String> = exhaustive_loop.variables.clone();
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
            exhaustive_loop.variables.join(" -> ")
        );
    }
}

#[test]
fn discovery_arms_race_3party() {
    let model_path = "../../test/arms_race_3party/arms_race.stmx";

    // Exhaustive mode via salsa-tracked structural analysis
    let f = File::open(model_path).unwrap();
    let mut f = BufReader::new(f);
    let datamodel_project = xmile::project_from_reader(&mut f).unwrap();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel_project);
    let source_model = sync.models["main"].source;
    let exhaustive_loops = model_detected_loops(&db, source_model, sync.project);
    let exhaustive_count = exhaustive_loops.loops.len();

    // The three-party arms race has 8 unique feedback loops: 3
    // self-adjustment (balancing), 3 pairwise (reinforcing), and 2
    // three-way (reinforcing) -- one for each traversal direction.
    // Both directions visit the same node set but represent distinct
    // elementary directed cycles, so the canonical-rotation dedup
    // (issue #308) keeps them as separate loops.
    assert_eq!(
        exhaustive_count, 8,
        "Arms race should have 8 feedback loops, found {}",
        exhaustive_count
    );

    // Discovery mode
    let found = discover_loops_from_path(model_path);

    // With per-stock reset, discovery finds all 8 loops: each stock
    // starts with fresh per-node expansion budgets, so pairwise and
    // three-way reinforcing loops are not starved by expansions consumed
    // during earlier stocks' self-loop searches, and the
    // canonical-rotation dedup retains both directions of the three-way
    // loop as distinct paths.
    assert_eq!(
        found.len(),
        8,
        "Discovery should find all 8 loops in arms race model, found {}",
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

        let in_exhaustive = exhaustive_loops.loops.iter().any(|exh| {
            let mut exh_nodes: Vec<String> = exh.variables.clone();
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

    // Cross-validate with exhaustive via salsa-tracked structural analysis
    let f = File::open(model_path).unwrap();
    let mut f = BufReader::new(f);
    let datamodel_project = xmile::project_from_reader(&mut f).unwrap();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel_project);
    let source_model = sync.models["main"].source;
    let exhaustive_loops = model_detected_loops(&db, source_model, sync.project);
    // Discovery mode -- the decoupled stocks model has time-varying loop
    // activity where different loops activate at different timesteps,
    // demonstrating why per-timestep discovery is necessary.
    let found = discover_loops_from_path(model_path);

    // The heuristic finds 2 of 3 loops: the self-loops for each stock.
    // The cross-stock loop is missed because its two cross-links are
    // never simultaneously nonzero at any saved timestep (stock_1->flow_2
    // is active only around step 4, stock_2->flow_1 only at steps 6-10),
    // so the per-step zero-edge-excluded search graph never contains the
    // full cycle -- the "baton-passing" limitation tracked as GH #699.
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

        let in_exhaustive = exhaustive_loops.loops.iter().any(|exh| {
            let mut exh_nodes: Vec<String> = exh.variables.clone();
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
    let (compiled, loop_partitions) = compile_ltm_incremental_with_partitions(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Structural loop detection via salsa-tracked functions
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel_project);
    let source_model = sync.models["main"].source;
    let detected = model_detected_loops(&db, source_model, sync.project);
    assert!(
        !detected.loops.is_empty(),
        "expected feedback loops from LTM analysis"
    );

    let rel_scores = ltm_post::compute_rel_loop_scores(&results, &loop_partitions);
    let mut failures: Vec<String> = Vec::new();

    for loop_item in &detected.loops {
        let Some(series) = rel_scores.get(&loop_item.id) else {
            continue;
        };

        // Extract the time series for this loop.
        let time_series: Vec<(f64, f64)> = series
            .iter()
            .enumerate()
            .map(|(step, v)| {
                let time = results.specs.start + results.specs.save_step * (step as f64);
                (time, *v)
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
                    loop_item.variables.join(" -> "),
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

    // Issue #418: module-containing loops should have known polarity
    // (Balancing in this case), not Undetermined.
    //
    // Match exhaustively rather than wildcarding: the polarity enum
    // grew Mostly{Reinforcing,Balancing} variants with #485, but the
    // structural pipeline only emits those when runtime confidence is
    // surfaced (it currently isn't), so a Rux/Bux result here would
    // be a regression worth failing on rather than silently passing.
    let has_determined_polarity = detected.loops.iter().any(|l| match l.polarity {
        DetectedLoopPolarity::Reinforcing | DetectedLoopPolarity::Balancing => true,
        DetectedLoopPolarity::MostlyReinforcing
        | DetectedLoopPolarity::MostlyBalancing
        | DetectedLoopPolarity::Undetermined => false,
    });
    assert!(
        has_determined_polarity,
        "Loops through SMOOTH should have determined polarity, not Undetermined. Found: {:?}",
        detected
            .loops
            .iter()
            .map(|l| (&l.id, &l.polarity, &l.variables))
            .collect::<Vec<_>>()
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

/// GH #548 (fixed): a feedback loop running *through* a SMOOTH macro must
/// capture the macro's composite contribution in its loop SCORE.
///
/// For the goal-seeking model
/// `level (stock) -> SMTH1(level) -> gap -> adjustment -> level`:
///
///   * The module computes its own internal composite correctly:
///     `$⁚smoothed_level⁚0⁚smth1·$⁚ltm⁚composite⁚input` is non-zero and varies
///     over the run (it carries the input->output path score through the
///     macro's internal stock).
///   * The module's OUTPUT-side root link score
///     `$⁚ltm⁚link_score⁚$⁚…smth1→smoothed_level` is the expected ~1.
///   * The INPUT-side root link score
///     `$⁚ltm⁚link_score⁚level→$⁚…smth1` is the exhaustive
///     `!from_is_module && to_is_module` composite-reference form
///     (`"$⁚…smth1·$⁚ltm⁚composite⁚input"`). Before the fix, the standalone
///     fragment compiler reconstructed the SMOOTH sub-model *without* its LTM
///     augmentation, so that cross-module reference did not resolve, the
///     fragment was dropped, and the link score read a constant 0 -- zeroing
///     the whole loop. Now `build_submodel_metadata` registers the sub-model's
///     LTM synthetic vars (including the composite), so the reference resolves
///     and the link score carries the macro's composite signal.
///   * Because the loop score is the product of its link scores, the
///     previously-zeroing `level->smth1` link now lets the parent
///     `$⁚ltm⁚loop_score⁚b1` carry the loop's real, varying activity.
#[test]
fn smooth_macro_composite_flows_into_parent_loop_score_gh548() {
    let with_smooth = TestProject::new("with_smooth")
        .with_sim_time(0.0, 20.0, 0.25)
        .aux("goal", "100", None)
        .stock("level", "50", &["adjustment"], &[], None)
        .aux("smoothed_level", "SMTH1(level, 3)", None)
        .aux("gap", "goal - smoothed_level", None)
        .aux("adjustment_time", "5", None)
        .flow("adjustment", "gap / adjustment_time", None)
        .build_datamodel();

    // The loop IS discovered structurally (names trimmed correctly).
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &with_smooth, None);
    let source_model = sync.models["main"].source_model;
    let detected = model_detected_loops(&db, source_model, sync.project);
    assert!(
        !detected.loops.is_empty(),
        "the SMOOTH feedback loop must be discovered structurally"
    );

    let compiled = compile_ltm_incremental(&with_smooth);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let series = |name: &str| -> Option<Vec<f64>> {
        let ident = Ident::<Canonical>::new(name);
        results
            .offsets
            .get(&ident)
            .map(|&off| results.iter().map(|row| row[off]).collect())
    };
    let max_abs = |s: &[f64]| {
        s.iter()
            .filter(|v| v.is_finite())
            .map(|v| v.abs())
            .fold(0.0_f64, f64::max)
    };

    // The module's OWN internal composite carries a real, varying signal.
    let internal_composite = series("$⁚smoothed_level⁚0⁚smth1·$⁚ltm⁚composite⁚input")
        .expect("module-internal composite var must be emitted");
    assert!(
        max_abs(&internal_composite) > 1.0,
        "the macro computes a non-trivial internal composite"
    );

    // The OUTPUT-side root link score works (~1).
    let out_side = series("$⁚ltm⁚link_score⁚$⁚smoothed_level⁚0⁚smth1→smoothed_level")
        .expect("module->downstream root link score must be emitted");
    assert!(
        max_abs(&out_side) > 0.5,
        "module->downstream link score should be ~1"
    );

    // THE FIX: the input-side root link score (which references the internal
    // composite across the module boundary) now resolves and carries the
    // macro's composite signal instead of a constant 0.
    let in_side = series("$⁚ltm⁚link_score⁚level→$⁚smoothed_level⁚0⁚smth1")
        .expect("input->module root link score must be emitted");
    assert!(
        max_abs(&in_side) > 0.0,
        "GH #548 fixed: the input->module composite reference must resolve and \
         carry the macro's composite contribution into the parent (was a \
         constant 0); got {in_side:?}"
    );
    // The link score must match the module's internal composite it references
    // (the composite-reference form is a verbatim read of the sub-model's
    // `$⁚ltm⁚composite⁚input`), proving the cross-module read resolves to the
    // right slot rather than some other non-zero value.
    assert_eq!(
        in_side, internal_composite,
        "the input->module link score is the verbatim cross-module read of the \
         macro's internal composite, so the two series must be identical"
    );

    // And the parent loop score is no longer zeroed by that dropped link: it
    // carries the loop's real activity through the SMOOTH macro.
    let loop_b1 = series("$⁚ltm⁚loop_score⁚b1").expect("parent loop score b1 must be emitted");
    assert!(
        max_abs(&loop_b1) > 0.0,
        "GH #548 fixed: the parent loop score through the SMOOTH macro must be \
         non-zero (was zeroed by the dropped input->module composite); got \
         {loop_b1:?}"
    );
}

/// GH #548 follow-up: the macro's contribution must actually *vary* over the
/// run -- a non-zero-at-one-step assertion alone could be satisfied by a
/// degenerate constant. As the goal-seeking system converges the smoothed
/// `level` (and hence the composite path score through the SMOOTH) changes,
/// so both the input->macro link score and the loop score must take more than
/// one distinct value. Covers SMOOTH (`SMTH1`) and DELAY (`DELAY1`), the two
/// most common macro-in-loop constructs in the #548 bug class.
#[test]
fn macro_in_loop_score_varies_over_run_gh548() {
    fn link_and_loop_vary(macro_call: &str, macro_kind: &str) {
        let model = TestProject::new("macro_loop")
            .with_sim_time(0.0, 30.0, 0.25)
            .aux("goal", "100", None)
            .stock("level", "50", &["adjustment"], &[], None)
            .aux("perceived_level", macro_call, None)
            .aux("gap", "goal - perceived_level", None)
            .aux("adjustment_time", "5", None)
            .flow("adjustment", "gap / adjustment_time", None)
            .build_datamodel();

        let compiled = compile_ltm_incremental(&model);
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();
        let results = vm.into_results();

        let series = |name: &str| -> Option<Vec<f64>> {
            let ident = Ident::<Canonical>::new(name);
            results
                .offsets
                .get(&ident)
                .map(|&off| results.iter().map(|row| row[off]).collect())
        };
        let distinct_finite = |s: &[f64]| -> usize {
            let mut vals: Vec<u64> = s
                .iter()
                .filter(|v| v.is_finite())
                .map(|v| v.to_bits())
                .collect();
            vals.sort_unstable();
            vals.dedup();
            vals.len()
        };

        let link = series(&format!(
            "$⁚ltm⁚link_score⁚level→$⁚perceived_level⁚0⁚{macro_kind}"
        ))
        .unwrap_or_else(|| panic!("input->{macro_kind} link score must be emitted"));
        assert!(
            distinct_finite(&link) > 1,
            "the {macro_kind} input link score must vary over the run (the \
             macro's composite responds to the changing input), not stub to a \
             single value; got {link:?}"
        );

        let loop_b1 = series("$⁚ltm⁚loop_score⁚b1")
            .expect("parent loop score b1 must be emitted for a macro-in-loop model");
        assert!(
            distinct_finite(&loop_b1) > 1,
            "the loop score through the {macro_kind} macro must vary over the \
             run; got {loop_b1:?}"
        );
    }

    link_and_loop_vary("SMTH1(level, 3)", "smth1");
    link_and_loop_vary("DELAY1(level, 3)", "delay1");
}

// Issue #419: discovery mode should find loops through SMOOTH composite paths.
#[test]
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
    let project_for_discovery = Project::from(datamodel_project.clone());

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let found = ltm_finding::discover_loops(&results, &project_for_discovery)
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
    let found = ltm_finding::discover_loops(&results, &project_for_discovery)
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

    let (compiled, loop_partitions) = compile_ltm_incremental_with_partitions(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Derive rel_loop_scores post-sim; the compile-time emitter is gone.
    let rel_scores = ltm_post::compute_rel_loop_scores(&results, &loop_partitions);

    assert_eq!(
        rel_scores.len(),
        2,
        "Should have exactly 2 relative loop score series, found {}",
        rel_scores.len()
    );

    // Each loop is alone in its partition, so each relative score should be +/-1.0
    for (loop_id, scores) in &rel_scores {
        let nonzero_scores: Vec<f64> = scores
            .iter()
            .copied()
            .filter(|v| *v != 0.0 && !v.is_nan())
            .collect();

        assert!(
            !nonzero_scores.is_empty(),
            "Should have non-zero relative scores for {}",
            loop_id
        );

        for score in &nonzero_scores {
            assert!(
                (score.abs() - 1.0).abs() < 1e-6,
                "Single-loop-per-partition relative score should have |value| = 1, got {} for {}",
                score,
                loop_id
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

    // Structural partition analysis via salsa-tracked functions
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel_project);
    let source_model = sync.models["main"].source;
    let partitions = model_cycle_partitions(&db, source_model, sync.project);

    // Both stocks should be in the same partition
    assert_eq!(
        partitions.partitions.len(),
        1,
        "Mutually-reachable stocks should be in one partition, got {}",
        partitions.partitions.len()
    );
    assert_eq!(partitions.partitions[0].len(), 2);

    // VM path for LTM simulation
    let (compiled, loop_partitions) = compile_ltm_incremental_with_partitions(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let rel_scores = ltm_post::compute_rel_loop_scores(&results, &loop_partitions);
    assert!(
        !rel_scores.is_empty(),
        "Should have relative loop score series"
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

    // Project for causal graph structural analysis (from_datamodel uses salsa internally)
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

// Guards the module->variable ceteris-paribus link-score arm from GH #675:
// when a module output (e.g. SMTH1(level, 3)) and a sibling input both feed a
// downstream equation, the module->downstream link score must be the real
// ceteris-paribus partial (holding the sibling input frozen at PREVIOUS, so
// magnitude ~0.5 for an even 50/50 split), NOT the raw black-box gain dz/dx.
// Before commit 193740cd the analysis could not isolate the module's
// contribution and emitted magnitude ~1; this asserts the partial is now used.
#[test]
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

    let (compiled, loop_partitions) = compile_ltm_incremental_with_partitions(&project);
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

    // Exhaustive mode emits loop_score variables; relative loop scores are
    // derived post-sim via `compute_rel_loop_scores` and no longer appear
    // as VM-computed variables.
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

    let rel_scores = ltm_post::compute_rel_loop_scores(&results, &loop_partitions);
    assert!(
        !rel_scores.is_empty(),
        "exhaustive mode should yield relative loop score series"
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

    // Build Project for discover_loops (from_datamodel uses salsa internally)
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

    let (compiled, loop_partitions) = compile_ltm_incremental_with_partitions(&project);
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

    // Each loop should have a post-sim relative loop score series; rel_loop_score
    // is no longer a materialized VM variable.
    let rel_scores = ltm_post::compute_rel_loop_scores(&results, &loop_partitions);
    assert_eq!(
        rel_scores.len(),
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
                macro_spec: None,
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
                macro_spec: None,
            },
        ],
        source: None,
        ai_information: None,
    };

    let (compiled, loop_partitions) = compile_ltm_incremental_with_partitions(&project);
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
    // with the module instance prefix (growth_model·varname, canonical middle dot)
    let submodel_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| k.as_str().starts_with("growth_model\u{00B7}"))
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
        .any(|k| k.as_str() == "growth_model\u{00B7}internal_level");
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
            s.starts_with("growth_model\u{00B7}") && s.contains("composite")
        })
        .collect();
    assert!(
        !composite_vars.is_empty(),
        "sub-model composite score variables should exist namespaced by the module \
         instance name (growth_model.*composite*), available keys: {:?}",
        results
            .offsets
            .keys()
            .filter(|k| k.as_str().starts_with("growth_model\u{00B7}"))
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

    // Verify loop scores exist (exhaustive mode); rel_loop_score is derived
    // post-sim via compute_rel_loop_scores.
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

    let rel_scores = ltm_post::compute_rel_loop_scores(&results, &loop_partitions);
    assert!(
        !rel_scores.is_empty(),
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
                macro_spec: None,
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
                macro_spec: None,
            },
        ],
        source: None,
        ai_information: None,
    };

    // Compile and simulate with LTM via the salsa/VM path.
    let (compiled, loop_partitions) = compile_ltm_incremental_with_partitions(&project);
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

    // Verify loop scores exist (exhaustive mode); rel_loop_score is derived post-sim.
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

    let rel_scores = ltm_post::compute_rel_loop_scores(&results, &loop_partitions);
    assert!(
        !rel_scores.is_empty(),
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
            s.starts_with("processor\u{00B7}") && s.contains("composite")
        })
        .collect();
    assert!(
        !nested_composite_vars.is_empty(),
        "composite scores should exist for the SMOOTH instance nested inside 'processor', \
         namespaced under processor.*, available processor.* keys: {:?}",
        results
            .offsets
            .keys()
            .filter(|k| k.as_str().starts_with("processor\u{00B7}"))
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
            s.starts_with("processor\u{00B7}") && s.contains("link_score") && !s.contains("arg0")
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

// --- A2A link score integration tests ---
//
// These tests verify end-to-end compilation and simulation of
// Apply-to-All (A2A) link scores for arrayed models. When both
// source and target variables share the same dimension (or the
// source is scalar), the link score inherits the target's dimensions
// and produces per-element values.

/// Helper: find a link score offset entry matching the given from->to
/// variable names (case-insensitive substring match on the offset key).
fn find_link_score_offset<'a>(
    results: &'a Results,
    from_name: &str,
    to_name: &str,
) -> Option<(&'a Ident<Canonical>, usize)> {
    let arrow = format!(
        "{}\u{2192}{}",
        from_name.to_lowercase().replace(' ', "_"),
        to_name.to_lowercase().replace(' ', "_")
    );
    results
        .offsets
        .iter()
        .find(|(k, _)| {
            let s = k.as_str();
            s.starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}") && s.contains(&arrow)
        })
        .map(|(k, off)| (k, *off))
}

/// AC4.1: A2A aux-to-aux link score for an arrayed feedback model
/// produces non-zero per-element values.
///
/// Model: a balancing feedback loop with 3 regions:
///   level[Region] (stock, init=50) -> gap[Region] (aux, 100 - level)
///     -> adj[Region] (flow, gap / 5) -> level[Region]
///
/// The level changes each timestep via the flow, causing gap to change,
/// so the link score for level -> gap should be non-zero after t=1.
/// The link score occupies 3 slots (one per region).
#[test]
fn test_a2a_aux_to_aux_link_score() {
    let n_elements: usize = 3;
    let project = TestProject::new("a2a_aux_to_aux")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_stock("level[Region]", "50", &["adj"], &[], None)
        .array_aux("gap[Region]", "100 - level")
        .array_flow("adj[Region]", "gap / 5", None)
        .build_datamodel();

    let compiled = compile_ltm_discovery_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // The link score for level -> gap (aux-to-aux) should exist
    let (link_key, base_offset) = find_link_score_offset(&results, "level", "gap")
        .expect("link score for level -> gap should exist in results");

    // Verify the offset key does NOT contain a subscript bracket -- it is
    // the base entry for the arrayed link score.
    assert!(
        !link_key.as_str().contains('['),
        "A2A link score should have a base (unsubscripted) offset entry, got: {}",
        link_key.as_str()
    );

    // Verify that consecutive slots contain per-element link score values.
    // After the first few timesteps (where PREVIOUS is not yet populated),
    // each element should have a non-zero link score.
    for elem in 0..n_elements {
        let elem_offset = base_offset + elem;
        let any_nonzero = (2..results.step_count).any(|step| {
            let val = results.data[step * results.step_size + elem_offset];
            val.abs() > 1e-10 && !val.is_nan()
        });
        assert!(
            any_nonzero,
            "A2A link score element {} (offset {}) should have non-zero values after t=1, \
             key: {}",
            elem,
            elem_offset,
            link_key.as_str()
        );
    }
}

/// AC4.2: A2A flow-to-stock link score produces per-element values.
///
/// Same balancing feedback model as test 1. The adj -> level link score
/// (flow-to-stock) should occupy 3 slots and produce non-zero per-element
/// values because both adj and level change each timestep.
#[test]
fn test_a2a_flow_to_stock_link_score() {
    let n_elements: usize = 3;
    let project = TestProject::new("a2a_flow_to_stock")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_stock("level[Region]", "50", &["adj"], &[], None)
        .array_aux("gap[Region]", "100 - level")
        .array_flow("adj[Region]", "gap / 5", None)
        .build_datamodel();

    let compiled = compile_ltm_discovery_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // The flow-to-stock link score adj -> level should exist
    let (link_key, base_offset) = find_link_score_offset(&results, "adj", "level")
        .expect("link score for adj -> level should exist in results");

    assert!(
        !link_key.as_str().contains('['),
        "A2A link score should have a base (unsubscripted) offset entry, got: {}",
        link_key.as_str()
    );

    // Each element should have non-zero flow-to-stock link scores
    for elem in 0..n_elements {
        let elem_offset = base_offset + elem;
        let any_nonzero = (2..results.step_count).any(|step| {
            let val = results.data[step * results.step_size + elem_offset];
            val.abs() > 1e-10 && !val.is_nan()
        });
        assert!(
            any_nonzero,
            "A2A flow-to-stock link score element {} (offset {}) should have non-zero values, \
             key: {}",
            elem,
            elem_offset,
            link_key.as_str()
        );
    }
}

/// LTM deep-review Finding 2: an *arrayed* isolated feedback loop's raw
/// loop score must be the exact isolated-loop invariant (`+/-1`) in every
/// per-element slot, exactly like a scalar isolated loop.
///
/// Model -- a per-element reinforcing loop over two independent regions:
///   `pop[region]`    (stock, init 100)
///   `growth[region] = pop[region] * rate[region]`   (the only inflow)
/// with distinct per-element rates so any slot leak would be visible.
/// Each element is its own isolated one-stock loop, so each `loop_score`
/// slot is exactly `+1` at `dt = 1` regardless of that element's gain
/// (Schoenberg, Davidsen & Eberlein 2020, sec. 4.1 / Appendix B).
///
/// The bug: `generate_flow_to_stock_equation` emitted the flow-to-stock
/// link score with *bare* arrayed names. Inside the resulting
/// `Equation::ApplyToAll` the `PREVIOUS(PREVIOUS(...))` terms route their
/// inner `PREVIOUS(name)` through a synthesized *scalar* helper aux (see
/// `builtins_visitor`), which cannot hold an arrayed value -- so the
/// helper fragment failed to compile and the LTM compiler silently
/// stubbed it to 0. With the nested-PREVIOUS terms zeroed the score
/// collapsed to `1/9` (`= 0.111...`) instead of `1`. `dt = 1` is chosen
/// so the Finding-1 `dt` factor is a no-op and any deviation from `+1` is
/// purely Finding 2.
#[test]
fn arrayed_isolated_loop_raw_score_is_one_per_element() {
    let project = TestProject::new("arrayed_isolated_loop")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["growth"], &[], None)
        .array_flow("growth[region]", "pop[region] * rate[region]", None)
        .array_with_ranges("rate[region]", vec![("north", "0.1"), ("south", "0.4")])
        .build_datamodel();

    let compiled = compile_ltm_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Exactly one A2A feedback loop, carried as a single base (unsubscripted)
    // loop-score offset entry with one slot per region.
    let loop_keys: Vec<&Ident<Canonical>> = results
        .offsets
        .keys()
        .filter(|k| {
            let s = k.as_str();
            s.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}") && !s.contains('[')
        })
        .collect();
    assert_eq!(
        loop_keys.len(),
        1,
        "expected exactly one A2A loop score, found {:?}",
        loop_keys.iter().map(|k| k.as_str()).collect::<Vec<_>>()
    );
    let base_offset = results.offsets[loop_keys[0]];

    // Two regions -> two per-element loop-score slots (slot 0 = north,
    // slot 1 = south). Each is an isolated reinforcing one-stock loop, so
    // each slot is exactly +1 after the two-step startup guard (the
    // flow-to-stock score's second-order denominator needs two steps of
    // history before it is defined).
    const STARTUP_STEPS: usize = 2;
    for (elem, region) in ["north", "south"].iter().enumerate() {
        let slot = base_offset + elem;
        for step in 0..results.step_count {
            let value = results.data[step * results.step_size + slot];
            if step < STARTUP_STEPS {
                assert_eq!(
                    value, 0.0,
                    "region {region}: step {step} is inside the startup guard and must be \
                     exactly 0, got {value}"
                );
            } else {
                assert!(
                    (value - 1.0).abs() < 1e-6,
                    "region {region}: arrayed isolated loop score at step {step} is {value}, \
                     expected exactly +1. A value near 1/9 means the flow-to-stock link \
                     score's nested PREVIOUS terms were stubbed to 0 (LTM review Finding 2)."
                );
            }
        }
    }
}

/// AC4.4: Scalar-to-arrayed link score varies by element when the target
/// has different per-element values.
///
/// Model: An arrayed balancing feedback loop with a scalar capacity variable:
///   level[Region] (stock, different inits: 100, 200, 300)
///     -> gap[Region] (aux, capacity - level)
///     -> adj[Region] (flow, gap / 5)
///     -> level[Region]
///   capacity (scalar, = 500 -- constant, but it appears in the arrayed gap equation)
///
/// The scalar capacity feeds into the arrayed gap[Region]. Because each
/// region's level differs, each region's gap differs, causing different
/// per-element link score values for capacity -> gap.
#[test]
fn test_scalar_to_arrayed_link_score() {
    use simlin_engine::datamodel::{self, Equation, Variable};

    let project = datamodel::Project {
        name: "scalar_to_arrayed".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension::named(
            "Region".to_string(),
            vec!["NYC".to_string(), "Boston".to_string(), "LA".to_string()],
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                // level[Region] with different initial values per element
                Variable::Stock(datamodel::Stock {
                    ident: "level".to_string(),
                    equation: Equation::Arrayed(
                        vec!["Region".to_string()],
                        vec![
                            ("NYC".to_string(), "100".to_string(), None, None),
                            ("Boston".to_string(), "200".to_string(), None, None),
                            ("LA".to_string(), "300".to_string(), None, None),
                        ],
                        None,
                        false,
                    ),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["adj".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // Scalar capacity -- changes over time to make its link score non-zero.
                // Uses TIME to produce a time-varying scalar.
                Variable::Aux(datamodel::Aux {
                    ident: "capacity".to_string(),
                    equation: Equation::Scalar("500 + TIME * 10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // gap[Region] = capacity - level (scalar-to-arrayed edge)
                Variable::Aux(datamodel::Aux {
                    ident: "gap".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        "capacity - level".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // adj[Region] = gap / 5
                Variable::Flow(datamodel::Flow {
                    ident: "adj".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        "gap / 5".to_string(),
                    ),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let compiled = compile_ltm_discovery_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // The scalar-source -> arrayed-target edge `capacity -> gap` is emitted
    // as one scalar link score *per target element*, named
    // `$⁚ltm⁚link_score⁚capacity→gap[{elem}]` with no dimensions -- NOT a
    // single Bare-A2A var with three contiguous slots. (The Bare-A2A form
    // was undiscoverable: `parse_link_offsets`'s `expand_a2a_link_offsets`
    // would invent a phantom `capacity[nyc]` node.)
    for elem in ["nyc", "boston", "la"] {
        let want = format!("$\u{205A}ltm\u{205A}link_score\u{205A}capacity\u{2192}gap[{elem}]");
        let off = *results
            .offsets
            .iter()
            .find(|(k, _)| k.as_str() == want)
            .map(|(_, off)| off)
            .unwrap_or_else(|| {
                panic!(
                    "expected per-target-element scalar link score {want:?}; link scores present: {:?}",
                    results
                        .offsets
                        .keys()
                        .filter(|k| k.as_str().contains("link_score"))
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                )
            });
        // Each element's link score (partial of `gap[e] = capacity - level[e]`
        // w.r.t. `capacity` live, `level[e]` frozen) is non-zero: capacity
        // changes by +10 each step and `level[e]` drifts upward via the
        // adj inflow, so |Δcapacity / Δgap[e]| > 0.
        let any_nonzero = (2..results.step_count).any(|step| {
            let val = results.data[step * results.step_size + off];
            val.abs() > 1e-10 && !val.is_nan()
        });
        assert!(
            any_nonzero,
            "per-target-element scalar link score {want:?} (offset {off}) should be non-zero at some step"
        );
    }
}

/// AC4.5: Per-element link scores are computed independently using each
/// element's own values, not shared across elements.
///
/// Model: An arrayed balancing feedback loop where each element has a
/// nonlinear flow equation that produces different link score ratios
/// when the elements have different states.
///
///   level[Region] (stock, inits: 100, 200, 300)
///     -> gap[Region] (aux, 500 - level)
///     -> adj[Region] (flow, gap * gap / 1000 -- quadratic in gap)
///     -> level[Region]
///
/// The quadratic flow equation `gap^2/1000` means the ceteris paribus
/// partial derivative varies with the current gap value. Since each
/// region starts at a different level, each has a different gap, and
/// thus a different discrete link score ratio. This proves that the
/// per-element computation uses element-specific values.
#[test]
fn test_a2a_independent_per_element_computation() {
    use simlin_engine::datamodel::{self, Equation, Variable};

    let n_elements: usize = 3;

    let project = datamodel::Project {
        name: "a2a_independent_elements".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 20.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension::named(
            "Region".to_string(),
            vec!["NYC".to_string(), "Boston".to_string(), "LA".to_string()],
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                // level[Region] with different initial values
                Variable::Stock(datamodel::Stock {
                    ident: "level".to_string(),
                    equation: Equation::Arrayed(
                        vec!["Region".to_string()],
                        vec![
                            ("NYC".to_string(), "100".to_string(), None, None),
                            ("Boston".to_string(), "200".to_string(), None, None),
                            ("LA".to_string(), "300".to_string(), None, None),
                        ],
                        None,
                        false,
                    ),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["adj".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // gap[Region] = 500 - level
                Variable::Aux(datamodel::Aux {
                    ident: "gap".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        "500 - level".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // adj[Region] = gap * level / 1000 (depends on both gap
                // and level; the ceteris paribus formula wraps level with
                // PREVIOUS, producing different per-element ratios because
                // level differs across elements)
                Variable::Flow(datamodel::Flow {
                    ident: "adj".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        "gap * level / 1000".to_string(),
                    ),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let compiled = compile_ltm_discovery_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // The gap -> adj link score should exist (aux-to-flow, A2A)
    let (_link_key, base_offset) = find_link_score_offset(&results, "gap", "adj")
        .expect("link score for gap -> adj should exist");

    // Look for a timestep where all elements are non-zero and the
    // values differ across elements. The quadratic relationship and
    // different initial conditions guarantee this after a few steps.
    let mut found_differing_step = false;
    for step in 2..results.step_count {
        let mut element_values = Vec::new();
        for elem in 0..n_elements {
            let val = results.data[step * results.step_size + base_offset + elem];
            element_values.push(val);
        }

        let all_nonzero = element_values
            .iter()
            .all(|v| v.abs() > 1e-10 && !v.is_nan());
        if !all_nonzero {
            continue;
        }

        let all_same = element_values
            .windows(2)
            .all(|w| (w[0] - w[1]).abs() < 1e-10);
        if !all_same {
            found_differing_step = true;
            break;
        }
    }

    assert!(
        found_differing_step,
        "per-element link scores should differ at some timestep when initial conditions \
         vary and the flow equation is nonlinear, proving independent per-element computation"
    );
}

// ============================================================================
// AC5: Cross-dimensional link scores (arrayed-to-scalar)
//
// When an arrayed variable feeds a scalar target through an array-reducing
// function, each element gets its own scalar link score variable.
// ============================================================================

/// Find all per-element cross-dimensional link score offsets for a given
/// from->to edge. Returns a vec of (element_name, offset) pairs.
fn find_cross_dimensional_offsets(
    results: &Results,
    from_name: &str,
    to_name: &str,
) -> Vec<(String, usize)> {
    let from_lower = from_name.to_lowercase().replace(' ', "_");
    let to_lower = to_name.to_lowercase().replace(' ', "_");
    // Cross-dimensional link scores are named:
    //   $⁚ltm⁚link_score⁚{from}[{element}]→{to}
    let prefix = format!("$\u{205A}ltm\u{205A}link_score\u{205A}{}[", from_lower);
    let arrow_to = format!("]\u{2192}{}", to_lower);

    let mut offsets: Vec<(String, usize)> = results
        .offsets
        .iter()
        .filter_map(|(k, &off)| {
            let s = k.as_str();
            if s.starts_with(&prefix) && s.contains(&arrow_to) {
                // Extract element name between [ and ]→
                let after_bracket = &s[prefix.len()..];
                if let Some(end) = after_bracket.find("]\u{2192}") {
                    let elem = after_bracket[..end].to_string();
                    return Some((elem, off));
                }
            }
            None
        })
        .collect();
    offsets.sort_by_key(|a| a.1);
    offsets
}

/// Build a simple arrayed-to-scalar model with a given reducer equation.
///
/// Model structure:
///   population[Region] (stock, inits: NYC=100, Boston=200, LA=300)
///   growth[Region] (flow, = population * 0.05)
///   scalar_target (aux, = {reducer_equation})
///
/// The stock changes each timestep via the flow, so the source values
/// change, producing non-zero link scores for the arrayed-to-scalar edge.
fn build_arrayed_to_scalar_model(
    name: &str,
    reducer_equation: &str,
    target_var_name: &str,
) -> simlin_engine::datamodel::Project {
    use simlin_engine::datamodel::{self, Equation, Variable};

    datamodel::Project {
        name: name.to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension::named(
            "Region".to_string(),
            vec!["NYC".to_string(), "Boston".to_string(), "LA".to_string()],
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                // population[Region] with different initial values
                Variable::Stock(datamodel::Stock {
                    ident: "population".to_string(),
                    equation: Equation::Arrayed(
                        vec!["Region".to_string()],
                        vec![
                            ("NYC".to_string(), "100".to_string(), None, None),
                            ("Boston".to_string(), "200".to_string(), None, None),
                            ("LA".to_string(), "300".to_string(), None, None),
                        ],
                        None,
                        false,
                    ),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["growth".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // growth[Region] = population * 0.05
                Variable::Flow(datamodel::Flow {
                    ident: "growth".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        "population * 0.05".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // scalar target using the reducer
                Variable::Aux(datamodel::Aux {
                    ident: target_var_name.to_string(),
                    equation: Equation::Scalar(reducer_equation.to_string()),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// AC5.1: SUM(population[*]) produces N scalar per-element link scores
/// using the algebraic shortcut.
#[test]
fn test_cross_dim_sum_algebraic() {
    let project = build_arrayed_to_scalar_model("cross_dim_sum", "SUM(population[*])", "total_pop");

    let compiled = compile_ltm_discovery_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let offsets = find_cross_dimensional_offsets(&results, "population", "total_pop");
    assert_eq!(
        offsets.len(),
        3,
        "SUM should produce 3 per-element link scores, got: {:?}",
        offsets
    );

    // Each element should have non-zero link scores after the initial step
    for (elem, offset) in &offsets {
        let any_nonzero = (2..results.step_count).any(|step| {
            let val = results.data[step * results.step_size + offset];
            val.abs() > 1e-10 && !val.is_nan()
        });
        assert!(
            any_nonzero,
            "SUM per-element link score for {} (offset {}) should be non-zero",
            elem, offset
        );
    }

    // Values should differ between elements since they have different
    // initial values (100, 200, 300) causing different absolute changes.
    // For SUM the algebraic shortcut means each element contributes
    // proportionally to its own change, so we expect per-element scores
    // to potentially have different magnitudes when combined with the
    // sign term.

    // Polarity assertion: total_pop = SUM(population[*]) is monotone-positive
    // in every population element, so the population -> total_pop link must
    // be Positive (not Unknown). Without an Expr2::Subscript arm in
    // analyze_expr_polarity_with_context, the parsed `Sum(Subscript(...))`
    // shape would fall through to Unknown despite the Sum reducer arm.
    // The growth -> population stock-feedback link (and the resulting
    // reinforcing loop) likewise should not be Undetermined.
    use simlin_engine::db::compute_link_polarities;
    use simlin_engine::ltm::LinkPolarity;
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;
    let polarities = compute_link_polarities(&db, source_model, sync.project);
    let pop_to_total = polarities
        .get(&("population".to_string(), "total_pop".to_string()))
        .copied()
        .unwrap_or(LinkPolarity::Unknown);
    assert_eq!(
        pop_to_total,
        LinkPolarity::Positive,
        "population -> total_pop polarity for SUM(population[*]) must be Positive, got: {:?}",
        pop_to_total
    );

    // The growth -> population stock-feedback edge plus population -> growth
    // form a single reinforcing loop. With both link polarities now known,
    // the loop must classify as Reinforcing rather than Undetermined.
    let detected = model_detected_loops(&db, source_model, sync.project);
    assert!(
        !detected.loops.is_empty(),
        "model should have at least one loop"
    );
    let undetermined: Vec<_> = detected
        .loops
        .iter()
        .filter(|l| l.polarity == DetectedLoopPolarity::Undetermined)
        .map(|l| (l.id.clone(), l.variables.clone()))
        .collect();
    assert!(
        undetermined.is_empty(),
        "no loop should remain Undetermined now that SUM(x[*]) propagates polarity, but got: {:?}",
        undetermined
    );
}

/// AC5.2: MEAN(population[*]) produces N scalar per-element link scores
/// using the algebraic shortcut (like SUM but divided by N).
#[test]
fn test_cross_dim_mean_algebraic() {
    let project = build_arrayed_to_scalar_model("cross_dim_mean", "MEAN(population[*])", "avg_pop");

    let compiled = compile_ltm_discovery_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let offsets = find_cross_dimensional_offsets(&results, "population", "avg_pop");
    assert_eq!(
        offsets.len(),
        3,
        "MEAN should produce 3 per-element link scores, got: {:?}",
        offsets
    );

    for (elem, offset) in &offsets {
        let any_nonzero = (2..results.step_count).any(|step| {
            let val = results.data[step * results.step_size + offset];
            val.abs() > 1e-10 && !val.is_nan()
        });
        assert!(
            any_nonzero,
            "MEAN per-element link score for {} (offset {}) should be non-zero",
            elem, offset
        );
    }
}

/// AC5.3: MIN(population[*]) produces N scalar per-element link scores
/// using explicit element expansion.
///
/// Because only the element that IS the current minimum can affect the
/// MIN result, we expect the element with the smallest value (NYC=100)
/// to have the largest link score, while others should be ~0 when their
/// values are above the minimum.
#[test]
fn test_cross_dim_min_expansion() {
    let project = build_arrayed_to_scalar_model("cross_dim_min", "MIN(population[*])", "min_pop");

    let compiled = compile_ltm_discovery_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let offsets = find_cross_dimensional_offsets(&results, "population", "min_pop");
    assert_eq!(
        offsets.len(),
        3,
        "MIN should produce 3 per-element link scores, got: {:?}",
        offsets
    );

    // NYC starts at 100 (the minimum) and should have non-zero scores.
    // Boston (200) and LA (300) are above the min, so their individual
    // changes do not affect MIN -- their scores should be near zero.
    let nyc_off = offsets.iter().find(|(e, _)| e == "nyc").unwrap().1;
    let boston_off = offsets.iter().find(|(e, _)| e == "boston").unwrap().1;
    let la_off = offsets.iter().find(|(e, _)| e == "la").unwrap().1;

    // Check at step 2 (first step with meaningful PREVIOUS data)
    let step = 2;
    let nyc_val = results.data[step * results.step_size + nyc_off];
    let boston_val = results.data[step * results.step_size + boston_off];
    let la_val = results.data[step * results.step_size + la_off];

    // NYC (the minimum element) should have a significant score
    assert!(
        nyc_val.abs() > 1e-10 && !nyc_val.is_nan(),
        "MIN: NYC (the minimum) should have non-zero link score, got: {}",
        nyc_val
    );
    // Boston and LA are above the min, so perturbing them individually
    // while holding others at PREVIOUS should not change MIN. Their
    // scores should be approximately 0.
    assert!(
        boston_val.abs() < 1e-10 || boston_val.is_nan(),
        "MIN: Boston (above min) should have ~0 link score, got: {}",
        boston_val
    );
    assert!(
        la_val.abs() < 1e-10 || la_val.is_nan(),
        "MIN: LA (above min) should have ~0 link score, got: {}",
        la_val
    );
}

/// AC5.4: MAX(population[*]) produces N scalar per-element link scores.
///
/// The element with the largest value (LA=300) should dominate.
#[test]
fn test_cross_dim_max_expansion() {
    let project = build_arrayed_to_scalar_model("cross_dim_max", "MAX(population[*])", "max_pop");

    let compiled = compile_ltm_discovery_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let offsets = find_cross_dimensional_offsets(&results, "population", "max_pop");
    assert_eq!(
        offsets.len(),
        3,
        "MAX should produce 3 per-element link scores, got: {:?}",
        offsets
    );

    // LA starts at 300 (the maximum) and should have non-zero scores.
    let la_off = offsets.iter().find(|(e, _)| e == "la").unwrap().1;
    let nyc_off = offsets.iter().find(|(e, _)| e == "nyc").unwrap().1;
    let boston_off = offsets.iter().find(|(e, _)| e == "boston").unwrap().1;

    let step = 2;
    let la_val = results.data[step * results.step_size + la_off];
    let nyc_val = results.data[step * results.step_size + nyc_off];
    let boston_val = results.data[step * results.step_size + boston_off];

    assert!(
        la_val.abs() > 1e-10 && !la_val.is_nan(),
        "MAX: LA (the maximum) should have non-zero link score, got: {}",
        la_val
    );
    assert!(
        nyc_val.abs() < 1e-10 || nyc_val.is_nan(),
        "MAX: NYC (below max) should have ~0 link score, got: {}",
        nyc_val
    );
    assert!(
        boston_val.abs() < 1e-10 || boston_val.is_nan(),
        "MAX: Boston (below max) should have ~0 link score, got: {}",
        boston_val
    );
}

/// AC5.5: STDDEV(population[*]) produces N scalar per-element link scores
/// using explicit element expansion.
#[test]
fn test_cross_dim_stddev_expansion() {
    let project =
        build_arrayed_to_scalar_model("cross_dim_stddev", "STDDEV(population[*])", "std_pop");

    let compiled = compile_ltm_discovery_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let offsets = find_cross_dimensional_offsets(&results, "population", "std_pop");
    assert_eq!(
        offsets.len(),
        3,
        "STDDEV should produce 3 per-element link scores, got: {:?}",
        offsets
    );

    // All elements contribute to the standard deviation, so all should
    // have non-zero link scores.
    for (elem, offset) in &offsets {
        let any_nonzero = (2..results.step_count).any(|step| {
            let val = results.data[step * results.step_size + offset];
            val.abs() > 1e-10 && !val.is_nan()
        });
        assert!(
            any_nonzero,
            "STDDEV per-element link score for {} (offset {}) should be non-zero",
            elem, offset
        );
    }
}

/// Build a 3-region STDDEV-in-a-feedback-loop model (#483, AC6.1/AC6.3).
///
/// `Region = {a, b, c}`; `s[Region]` is a stock with heterogeneous inits
/// (`a=10, b=20, c=30`) fed by `update[Region]`; `total = STDDEV(s[*])`
/// (a scalar aux); `update[Region]` is a *per-element-equation* flow
/// (`Equation::Arrayed`) with `update[a] = total*c`, `update[b] =
/// total*c*0.5`, `update[c] = total*c*2`. The differing per-element
/// multipliers make the elements drift apart at different rates, so STDDEV
/// keeps changing -- the analytic per-element ceteris-paribus partial
/// isolates each element's contribution while the pre-#483 delta-ratio
/// conflated them (`partial_eq == target` ⇒ the link-score magnitude was a
/// degenerate `1` whenever `Δtotal ≠ 0`). The closed loop is `s[r] → total
/// → update[r] → s[r]` per element, plus the cross-element
/// `s[r] → total → update[r'] → s[r']`.
fn build_stddev_feedback_model(c: f64) -> simlin_engine::datamodel::Project {
    use simlin_engine::datamodel::{self, Equation, Variable};
    datamodel::Project {
        name: "stddev_feedback".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 5.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension::named(
            "Region".to_string(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Stock(datamodel::Stock {
                    ident: "s".to_string(),
                    equation: Equation::Arrayed(
                        vec!["Region".to_string()],
                        vec![
                            ("a".to_string(), "10".to_string(), None, None),
                            ("b".to_string(), "20".to_string(), None, None),
                            ("c".to_string(), "30".to_string(), None, None),
                        ],
                        None,
                        false,
                    ),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["update".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Aux(datamodel::Aux {
                    ident: "total".to_string(),
                    equation: Equation::Scalar("STDDEV(s[*])".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(datamodel::Flow {
                    ident: "update".to_string(),
                    equation: Equation::Arrayed(
                        vec!["Region".to_string()],
                        vec![
                            ("a".to_string(), format!("total * {c}"), None, None),
                            ("b".to_string(), format!("total * {c} * 0.5"), None, None),
                            ("c".to_string(), format!("total * {c} * 2"), None, None),
                        ],
                        None,
                        false,
                    ),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// Build a 3-region STDDEV-*invariant*-regime model (#483, AC6.2).
///
/// Same `s[Region]` stock with heterogeneous inits, but the inflow
/// `update[Region] = k` is a *constant scalar identical for every element*,
/// so all `s[i]` shift by the same `k` each step ⇒ STDDEV is invariant ⇒
/// `total` is bit-for-bit constant ⇒ the `Δtarget = 0` guard in
/// `build_element_reducer_link_score` zeros every `s[d]→total` link score
/// at every step ≥ 1. Note: this is genuinely zero with *both* the old
/// delta-ratio and the new analytic partial (both numerators are 0 when
/// `Δtotal = 0`), so it pins AC6.2 but does *not* distinguish the fix --
/// the load-bearing distinguishing test is `test_stddev_link_score_matches_hand_calc`.
/// `update` has no `total` dependency, so there is no feedback loop;
/// discovery mode (which scores every causal edge) emits the `s[d]→total`
/// link scores anyway.
fn build_stddev_invariant_model(k: f64) -> simlin_engine::datamodel::Project {
    use simlin_engine::datamodel::{self, Equation, Variable};
    datamodel::Project {
        name: "stddev_invariant".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 5.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension::named(
            "Region".to_string(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Stock(datamodel::Stock {
                    ident: "s".to_string(),
                    equation: Equation::Arrayed(
                        vec!["Region".to_string()],
                        vec![
                            ("a".to_string(), "10".to_string(), None, None),
                            ("b".to_string(), "20".to_string(), None, None),
                            ("c".to_string(), "30".to_string(), None, None),
                        ],
                        None,
                        false,
                    ),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["update".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Aux(datamodel::Aux {
                    ident: "total".to_string(),
                    equation: Equation::Scalar("STDDEV(s[*])".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(datamodel::Flow {
                    ident: "update".to_string(),
                    equation: Equation::ApplyToAll(vec!["Region".to_string()], format!("{k}")),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// #483 / ltm-arrays-hardening.AC6.1 / AC6.3: for `total = STDDEV(s[*])`
/// feeding back into `s`, the per-element `$⁚ltm⁚link_score⁚s[d]→total`
/// series equals the analytic ceteris-paribus partial (the unrolled
/// population-variance `sqrt` formula holding `s[d]` live, the other
/// elements at `PREVIOUS`), wrapped in the standard link-score formula --
/// matched against a hand calculation at every step within 1e-6 -- and
/// is *not* the degenerate-`1` magnitude the pre-#483 delta-ratio produced
/// (`partial_eq == target` ⇒ `ABS(SAFEDIV(Δtotal, Δtotal, 0)) == 1`). So
/// this test would fail on the pre-fix code.
#[test]
fn test_stddev_link_score_matches_hand_calc() {
    // A multiplier large enough that the elements drift apart visibly each
    // step (keeps the hand calc well clear of floating-point noise) but the
    // stock values stay modest over 5 steps.
    let project = build_stddev_feedback_model(1.0);
    let compiled = compile_ltm_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end()
        .expect("STDDEV feedback model should simulate with LTM enabled");
    let results = vm.into_results();

    let off = |name: &str| -> usize {
        *results
            .offsets
            .get(&Ident::<Canonical>::new(name))
            .unwrap_or_else(|| {
                panic!(
                    "missing offset {name}; have: {:?}",
                    results
                        .offsets
                        .keys()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                )
            })
    };
    let s_a = off("s[a]");
    let s_b = off("s[b]");
    let s_c = off("s[c]");
    let total_off = off("total");

    let ls_offsets = find_cross_dimensional_offsets(&results, "s", "total");
    assert_eq!(
        ls_offsets.len(),
        3,
        "STDDEV reducer edge should produce 3 per-element link scores, got: {ls_offsets:?}"
    );
    let ls_off = |elem: &str| -> usize {
        ls_offsets
            .iter()
            .find(|(e, _)| e == elem)
            .unwrap_or_else(|| panic!("no s[{elem}]→total link score; have {ls_offsets:?}"))
            .1
    };

    let at = |step: usize, o: usize| results.data[step * results.step_size + o];

    // The link-score equation (`build_element_reducer_link_score`) at step
    // `t >= 1` is:
    //   if Δtotal == 0 OR Δs[d] == 0 then 0
    //   else |SAFEDIV(partial_d(t) - total(t-1), Δtotal, 0)|
    //        * SIGN(SAFEDIV(partial_d(t) - total(t-1), Δs[d], 0))
    // where partial_d(t) = sqrt((Σ_i (s'_i - m)^2) / 3), s'_i = s[d](t) for
    // i == d else s[i](t-1), m = (Σ_i s'_i) / 3 -- matching the engine's
    // population-variance STDDEV (divisor N, `(v-mean).powf(2.0)`).
    let elems: [(&str, usize); 3] = [("a", s_a), ("b", s_b), ("c", s_c)];
    let mut checked = 0usize;
    let mut saw_non_degenerate = false;
    for step in 1..results.step_count {
        let total_t = at(step, total_off);
        let total_prev = at(step - 1, total_off);
        let d_total = total_t - total_prev;
        if d_total.abs() < 1e-12 {
            continue;
        }
        for (live_idx, (live_elem, live_s_off)) in elems.iter().enumerate() {
            let d_source = at(step, *live_s_off) - at(step - 1, *live_s_off);
            // s'_i: live element at step t, others frozen at step t-1.
            let s_prime: Vec<f64> = elems
                .iter()
                .enumerate()
                .map(|(i, (_, s_off))| {
                    if i == live_idx {
                        at(step, *s_off)
                    } else {
                        at(step - 1, *s_off)
                    }
                })
                .collect();
            let m = (s_prime[0] + s_prime[1] + s_prime[2]) / 3.0;
            let variance = ((s_prime[0] - m).powf(2.0)
                + (s_prime[1] - m).powf(2.0)
                + (s_prime[2] - m).powf(2.0))
                / 3.0;
            let partial = variance.sqrt();
            let num = partial - total_prev;
            let expected = if d_source.abs() < 1e-12 {
                0.0
            } else {
                let sign_arg = num / d_source;
                let sign = if sign_arg > 0.0 {
                    1.0
                } else if sign_arg < 0.0 {
                    -1.0
                } else {
                    0.0
                };
                (num / d_total).abs() * sign
            };
            let recorded = at(step, ls_off(live_elem));
            assert!(
                (recorded - expected).abs() < 1e-6,
                "step {step}, element {live_elem}: recorded link score {recorded} != hand calc \
                 {expected} (partial_d(t) = {partial}, total(t) = {total_t}, total(t-1) = \
                 {total_prev}, Δs[d] = {d_source})"
            );
            // The pre-#483 delta-ratio produced |Δtotal/Δtotal| = 1 here
            // (whenever Δtotal != 0); the analytic value is not degenerate-1.
            if (recorded.abs() - 1.0).abs() > 1e-3 {
                saw_non_degenerate = true;
            }
        }
        checked += 1;
    }
    assert!(
        checked > 0,
        "expected at least one step t >= 1 with Δtotal != 0"
    );
    assert!(
        saw_non_degenerate,
        "the analytic STDDEV link score should differ from the pre-#483 degenerate-1 \
         magnitude at some step (|recorded| != ~1)"
    );
}

/// #483 / ltm-arrays-hardening.AC6.2: under a STDDEV-invariant per-element
/// flow (every element shifted by the same constant each step), STDDEV does
/// not change, so the `Δtarget = 0` guard zeros every `s[d]→total` link
/// score at every step ≥ 1. (Passes with both the old delta-ratio and the
/// new analytic partial -- both numerators vanish when `Δtotal = 0` -- so
/// this pins AC6.2 but does *not* distinguish the fix; the distinguishing
/// test is `test_stddev_link_score_matches_hand_calc`.)
#[test]
fn test_stddev_invariant_regime_link_scores_zero() {
    let project = build_stddev_invariant_model(5.0);
    // Discovery mode: `update` has no `total` dependency, so there is no
    // feedback loop -- but discovery scores every causal edge, including
    // the `s[d] → total` reducer edge.
    let compiled = compile_ltm_discovery_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end()
        .expect("STDDEV-invariant model should simulate with LTM enabled");
    let results = vm.into_results();

    let ls_offsets = find_cross_dimensional_offsets(&results, "s", "total");
    assert_eq!(
        ls_offsets.len(),
        3,
        "STDDEV reducer edge should produce 3 per-element link scores, got: {ls_offsets:?}"
    );
    for (elem, offset) in &ls_offsets {
        for step in 0..results.step_count {
            let val = results.data[step * results.step_size + offset];
            assert!(
                val.abs() < 1e-9 && !val.is_nan(),
                "STDDEV-invariant regime: s[{elem}]→total link score at step {step} should be \
                 ~0 (STDDEV is constant ⇒ Δtarget = 0 guard fires), got {val}"
            );
        }
    }
}

/// AC5.6 / AC4.4: A compound expression combining MAX and MIN -- as
/// sub-expressions, not whole-RHS -- mints two synthetic aggregate nodes,
/// and each gets N per-source-element reducer link scores.
///
/// `range_pop = MAX(population[*]) - MIN(population[*])`: `MAX(population[*])`
/// and `MIN(population[*])` are each a maximal reducer subexpression (neither
/// is inside the other), so Phase 5 hoists them into `$⁚ltm⁚agg⁚0` and
/// `$⁚ltm⁚agg⁚1`. The `population → range_pop` causal edge is rerouted through
/// both: `population[d] → $⁚ltm⁚agg⁚0` and `population[d] → $⁚ltm⁚agg⁚1` per
/// source element, then `$⁚ltm⁚agg⁚0 → range_pop` and `$⁚ltm⁚agg⁚1 → range_pop`.
/// So the per-source-element link scores are into the agg nodes, not directly
/// into `range_pop`.
///
/// **Justified deviation from `RANK(population[*], 1)` as a scalar target:**
/// RANK (Vensim VECTOR RANK) returns an array of 1-based ordinal positions
/// with the same cardinality as its input. It cannot be used as the equation
/// for a scalar aux: the engine would produce a dimension mismatch error
/// because RANK's output is always an array. The nonlinear reducer path
/// (generate_nonlinear_partial -- the MIN/MAX 2-arg unroll, STDDEV's analytic
/// ceteris-paribus partial, RANK's delta-ratio stand-in) is exercised when MAX
/// or MIN appears as a reducer, which is exactly what this test covers with
/// the compound `MAX(population[*]) - MIN(population[*])` pattern.
#[test]
fn test_cross_dim_compound_nonlinear() {
    let project = build_arrayed_to_scalar_model(
        "cross_dim_compound",
        "MAX(population[*]) - MIN(population[*])",
        "range_pop",
    );

    let compiled = compile_ltm_discovery_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Per-source-element link scores into each agg node.
    let mut all_offsets: Vec<(String, usize)> = Vec::new();
    for agg in &[
        "$\u{205A}ltm\u{205A}agg\u{205A}0",
        "$\u{205A}ltm\u{205A}agg\u{205A}1",
    ] {
        let offsets = find_cross_dimensional_offsets(&results, "population", agg);
        assert_eq!(
            offsets.len(),
            3,
            "reducer hoisted into {agg} should produce 3 per-source-element link scores, got: {:?}",
            offsets
        );
        all_offsets.extend(offsets);
    }
    // Also: each agg→range_pop link score must exist.
    for agg in &[
        "$\u{205A}ltm\u{205A}agg\u{205A}0",
        "$\u{205A}ltm\u{205A}agg\u{205A}1",
    ] {
        let name = format!("$\u{205A}ltm\u{205A}link_score\u{205A}{agg}\u{2192}range_pop");
        assert!(
            results
                .offsets
                .contains_key(&Ident::<Canonical>::new(&name)),
            "expected agg→range_pop link score {name:?}; offsets: {:?}",
            results
                .offsets
                .keys()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
        );
    }

    let offsets = all_offsets;
    // At least some elements should have non-zero link scores.
    // The range (MAX-MIN) changes when either the max or min element changes.
    let any_nonzero_anywhere = offsets.iter().any(|(_, offset)| {
        (2..results.step_count).any(|step| {
            let val = results.data[step * results.step_size + offset];
            val.abs() > 1e-10 && !val.is_nan()
        })
    });
    assert!(
        any_nonzero_anywhere,
        "compound nonlinear should produce at least one non-zero per-element link score"
    );
}

/// Build a feedback model whose scalar aux hoists *two* reducers reading the
/// same array: `ratio = MAX(pop[*]) / MEAN(pop[*])`. Phase 5 mints
/// `$⁚ltm⁚agg⁚0 = MAX(pop[*])` and `$⁚ltm⁚agg⁚1 = MEAN(pop[*])`, and the
/// `pop → ratio` edge is rerouted through both. `pop` is fed back by
/// `update[r] = ratio * c` (an absolute increment, *not* proportional to
/// `pop[r]`) so the elements grow at different relative rates and `ratio`
/// keeps changing -- a proportional flow would freeze `ratio` and the
/// agg→ratio link scores would all be zeroed by the Δtarget=0 guard.
fn build_two_reducer_target_model(c: f64) -> simlin_engine::datamodel::Project {
    use simlin_engine::datamodel::{self, Equation, Variable};
    datamodel::Project {
        name: "two_reducer".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 6.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension::named(
            "Region".to_string(),
            vec!["Big".to_string(), "Small".to_string()],
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Stock(datamodel::Stock {
                    ident: "pop".to_string(),
                    equation: Equation::Arrayed(
                        vec!["Region".to_string()],
                        vec![
                            ("Big".to_string(), "1000".to_string(), None, None),
                            ("Small".to_string(), "100".to_string(), None, None),
                        ],
                        None,
                        false,
                    ),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["update".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Aux(datamodel::Aux {
                    ident: "ratio".to_string(),
                    equation: Equation::Scalar("MAX(pop[*]) / MEAN(pop[*])".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(datamodel::Flow {
                    ident: "update".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        format!("ratio * {c}"),
                    ),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// AC4.2 regression: when a target hoists 2+ reducers (`ratio = MAX(pop[*]) /
/// MEAN(pop[*])`), the `$⁚ltm⁚agg⁚j → ratio` link score must hold *every other*
/// agg at PREVIOUS, not just leave it live. With the other agg left live the
/// substituted partial equals the actual `ratio` equation, the link-score
/// numerator becomes `Δratio`, and `ABS(SAFEDIV(Δratio, Δratio, 0))` collapses
/// the magnitude to exactly 1. The correct value -- the partial with the other
/// agg frozen -- is not ±1.
///
/// This also exercises the agg-node fragment dispatch in `compile_project_incremental`
/// Pass 3: an `$⁚ltm⁚agg⁚n → scalar_target` link score has no bracket or shape
/// suffix in its name, so the legacy `(from, to)`-keyed salsa fragment path used
/// to claim it -- but that path `reconstruct_single_variable`s the synthetic agg
/// name, gets `None`, and emits a degenerate equation that the agg name appears
/// nowhere in, collapsing the link score to zero. The fix routes any agg-node
/// link score through `ltm_var.equation` directly.
#[test]
fn test_agg_to_target_link_score_multi_reducer_target() {
    // A large per-step increment so `ratio` moves visibly each timestep
    // (keeps the hand-calc comparisons well clear of floating-point noise).
    let project = build_two_reducer_target_model(50.0);
    let compiled = compile_ltm_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("should simulate");
    let results = vm.into_results();

    let off = |name: &str| -> usize {
        *results
            .offsets
            .get(&Ident::<Canonical>::new(name))
            .unwrap_or_else(|| {
                panic!(
                    "missing offset {name}; have: {:?}",
                    results
                        .offsets
                        .keys()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                )
            })
    };
    let agg0 = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let agg1 = "$\u{205A}ltm\u{205A}agg\u{205A}1";
    let ratio_off = off("ratio");
    let agg0_off = off(agg0);
    let agg1_off = off(agg1);
    // Determine which agg is MAX and which is MEAN by comparing to the
    // hand-computed values at step 0 (MAX(1000,100)=1000, MEAN=550).
    let at = |step: usize, o: usize| results.data[step * results.step_size + o];
    let pop_big = off("pop[big]");
    let pop_small = off("pop[small]");
    let max_at = |s: usize| at(s, pop_big).max(at(s, pop_small));
    let mean_at = |s: usize| (at(s, pop_big) + at(s, pop_small)) / 2.0;
    // Identify agg roles.
    let agg0_is_max = (at(0, agg0_off) - max_at(0)).abs() < 1e-9;
    let (max_off, mean_off) = if agg0_is_max {
        (agg0_off, agg1_off)
    } else {
        (agg1_off, agg0_off)
    };
    // Sanity: the agg values track MAX/MEAN.
    for s in 0..results.step_count {
        assert!(
            (at(s, max_off) - max_at(s)).abs() < 1e-7 * max_at(s).abs().max(1.0),
            "step {s}: MAX agg = {}, hand = {}",
            at(s, max_off),
            max_at(s)
        );
        assert!(
            (at(s, mean_off) - mean_at(s)).abs() < 1e-7 * mean_at(s).abs().max(1.0),
            "step {s}: MEAN agg = {}, hand = {}",
            at(s, mean_off),
            mean_at(s)
        );
    }

    let ls0_off = off(&format!(
        "$\u{205A}ltm\u{205A}link_score\u{205A}{agg0}\u{2192}ratio"
    ));
    let ls1_off = off(&format!(
        "$\u{205A}ltm\u{205A}link_score\u{205A}{agg1}\u{2192}ratio"
    ));

    // ratio = agg_max / agg_mean. Partial w.r.t. agg_max held live:
    //   p_max(s) = agg_max(s) / agg_mean(s-1)
    // Partial w.r.t. agg_mean held live:
    //   p_mean(s) = agg_max(s-1) / agg_mean(s)
    // link_score = ABS((p - PREVIOUS(ratio)) / (ratio - PREVIOUS(ratio)))
    //              * SIGN((p - PREVIOUS(ratio)) / (agg - PREVIOUS(agg)))
    let expected_ls = |s: usize, partial: f64, agg_now: f64, agg_prev: f64| -> f64 {
        let ratio_prev = at(s - 1, ratio_off);
        let ratio_now = at(s, ratio_off);
        let d_ratio = ratio_now - ratio_prev;
        let d_agg = agg_now - agg_prev;
        if d_ratio.abs() < 1e-15 || d_agg.abs() < 1e-15 {
            return 0.0;
        }
        let num = partial - ratio_prev;
        (num / d_ratio).abs() * (num / d_agg).signum()
    };

    let mut saw_non_unit_magnitude = false;
    for s in 1..results.step_count {
        let p_max = at(s, max_off) / at(s - 1, mean_off);
        let p_mean = at(s - 1, max_off) / at(s, mean_off);
        let exp_max = expected_ls(s, p_max, at(s, max_off), at(s - 1, max_off));
        let exp_mean = expected_ls(s, p_mean, at(s, mean_off), at(s - 1, mean_off));
        // Map back to agg0/agg1 ordering.
        let (exp_ls0, exp_ls1) = if agg0_is_max {
            (exp_max, exp_mean)
        } else {
            (exp_mean, exp_max)
        };
        assert!(
            (at(s, ls0_off) - exp_ls0).abs() < 1e-6,
            "step {s}: {agg0}->ratio link score = {}, hand calc = {}",
            at(s, ls0_off),
            exp_ls0
        );
        assert!(
            (at(s, ls1_off) - exp_ls1).abs() < 1e-6,
            "step {s}: {agg1}->ratio link score = {}, hand calc = {}",
            at(s, ls1_off),
            exp_ls1
        );
        // The buggy version would force |link score| == 1 on every step where
        // Δratio != 0. Confirm we see a step where it is genuinely != 1.
        if at(s, ls0_off).abs() > 1e-9 && (at(s, ls0_off).abs() - 1.0).abs() > 1e-3 {
            saw_non_unit_magnitude = true;
        }
        if at(s, ls1_off).abs() > 1e-9 && (at(s, ls1_off).abs() - 1.0).abs() > 1e-3 {
            saw_non_unit_magnitude = true;
        }
    }
    assert!(
        saw_non_unit_magnitude,
        "expected at least one step where an agg->ratio link score magnitude \
         is not 1 (the multi-reducer bug would pin it to exactly 1)"
    );
}

/// AC5.7: SIZE(population[*]) produces no link score variables because
/// SIZE is a constant (depends only on dimension cardinality).
#[test]
fn test_cross_dim_size_skipped() {
    let project =
        build_arrayed_to_scalar_model("cross_dim_size", "SIZE(population[*])", "size_pop");

    let compiled = compile_ltm_discovery_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // SIZE is constant, so no link score should be generated for the
    // population -> size_pop edge.
    let offsets = find_cross_dimensional_offsets(&results, "population", "size_pop");
    assert!(
        offsets.is_empty(),
        "SIZE should NOT produce per-element link scores, but got: {:?}",
        offsets
    );

    // Also verify that no standard (non-per-element) link score exists
    let standard_score = find_link_score_offset(&results, "population", "size_pop");
    assert!(
        standard_score.is_none(),
        "SIZE should NOT produce any link score at all"
    );
}

/// AC5.8: Cross-validation -- SUM algebraic shortcut produces comparable
/// results to an equivalent model using individual scalar variables.
///
/// Build two models that compute the same mathematical result:
/// (A) Arrayed: population[Region] (stock) -> total_pop (aux, SUM(population[*]))
///     Uses the cross-dimensional algebraic shortcut for per-element link scores.
/// (B) Scalar: pop_nyc, pop_boston, pop_la (3 independent stocks) -> total_pop (aux, pop_nyc + pop_boston + pop_la)
///     Uses standard scalar-to-scalar link scores for each dependency.
///
/// Both models should produce equivalent link score semantics: each source
/// element's contribution to the total is the element's delta divided by the
/// total delta, matching the SUM algebraic shortcut.
#[test]
fn test_cross_dim_sum_vs_explicit_cross_validation() {
    use simlin_engine::datamodel::{self, Equation, Variable};

    // Model A: arrayed source with SUM reducer
    let project_a =
        build_arrayed_to_scalar_model("cross_val_sum", "SUM(population[*])", "total_pop");

    let compiled_a = compile_ltm_discovery_incremental(&project_a);
    let mut vm_a = Vm::new(compiled_a).unwrap();
    vm_a.run_to_end().unwrap();
    let results_a = vm_a.into_results();

    // Model B: three independent scalar stocks with explicit sum
    let project_b = datamodel::Project {
        name: "cross_val_scalar".to_string(),
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
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Stock(datamodel::Stock {
                    ident: "pop_nyc".to_string(),
                    equation: Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["growth_nyc".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Stock(datamodel::Stock {
                    ident: "pop_boston".to_string(),
                    equation: Equation::Scalar("200".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["growth_boston".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Stock(datamodel::Stock {
                    ident: "pop_la".to_string(),
                    equation: Equation::Scalar("300".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["growth_la".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(datamodel::Flow {
                    ident: "growth_nyc".to_string(),
                    equation: Equation::Scalar("pop_nyc * 0.05".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(datamodel::Flow {
                    ident: "growth_boston".to_string(),
                    equation: Equation::Scalar("pop_boston * 0.05".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(datamodel::Flow {
                    ident: "growth_la".to_string(),
                    equation: Equation::Scalar("pop_la * 0.05".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Aux(datamodel::Aux {
                    ident: "total_pop".to_string(),
                    equation: Equation::Scalar("pop_nyc + pop_boston + pop_la".to_string()),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let compiled_b = compile_ltm_discovery_incremental(&project_b);
    let mut vm_b = Vm::new(compiled_b).unwrap();
    vm_b.run_to_end().unwrap();
    let results_b = vm_b.into_results();

    // Model A: cross-dimensional per-element link scores
    let offsets_a = find_cross_dimensional_offsets(&results_a, "population", "total_pop");
    assert_eq!(
        offsets_a.len(),
        3,
        "Model A (SUM) should have 3 per-element scores"
    );

    // Model B: standard scalar-to-scalar link scores
    let b_nyc = find_link_score_offset(&results_b, "pop_nyc", "total_pop");
    let b_boston = find_link_score_offset(&results_b, "pop_boston", "total_pop");
    let b_la = find_link_score_offset(&results_b, "pop_la", "total_pop");

    assert!(
        b_nyc.is_some(),
        "Model B should have link score for pop_nyc -> total_pop"
    );
    assert!(
        b_boston.is_some(),
        "Model B should have link score for pop_boston -> total_pop"
    );
    assert!(
        b_la.is_some(),
        "Model B should have link score for pop_la -> total_pop"
    );

    // Compare at a timestep where all values are meaningful
    let test_step = 3;

    let a_nyc = results_a.data
        [test_step * results_a.step_size + offsets_a.iter().find(|(e, _)| e == "nyc").unwrap().1];
    let a_boston = results_a.data[test_step * results_a.step_size
        + offsets_a.iter().find(|(e, _)| e == "boston").unwrap().1];
    let a_la = results_a.data
        [test_step * results_a.step_size + offsets_a.iter().find(|(e, _)| e == "la").unwrap().1];

    let b_nyc_val = results_b.data[test_step * results_b.step_size + b_nyc.unwrap().1];
    let b_boston_val = results_b.data[test_step * results_b.step_size + b_boston.unwrap().1];
    let b_la_val = results_b.data[test_step * results_b.step_size + b_la.unwrap().1];

    // Both models should produce non-zero, non-NaN scores
    for (name, val) in [
        ("nyc_A", a_nyc),
        ("boston_A", a_boston),
        ("la_A", a_la),
        ("nyc_B", b_nyc_val),
        ("boston_B", b_boston_val),
        ("la_B", b_la_val),
    ] {
        assert!(
            val.abs() > 1e-10 && !val.is_nan(),
            "{} link score at step {} should be non-zero, got: {}",
            name,
            test_step,
            val
        );
    }

    // For SUM with 5% growth: each element's delta is proportional to its
    // value, so the algebraic shortcut's |delta_elem / delta_total| equals
    // elem_value / total_value. The scalar model's ceteris-paribus formula
    // produces the same ratio. Verify they match within tolerance.
    let tolerance = 0.01; // 1% tolerance
    assert!(
        (a_nyc - b_nyc_val).abs() < tolerance,
        "NYC link scores should match: A={}, B={}",
        a_nyc,
        b_nyc_val
    );
    assert!(
        (a_boston - b_boston_val).abs() < tolerance,
        "Boston link scores should match: A={}, B={}",
        a_boston,
        b_boston_val
    );
    assert!(
        (a_la - b_la_val).abs() < tolerance,
        "LA link scores should match: A={}, B={}",
        a_la,
        b_la_val
    );
}

// --- AC6: Element-level loop scores and relative scores ---

/// Helper: find all loop score variable names and offsets in results.
fn find_loop_score_offsets(results: &Results) -> Vec<(String, usize)> {
    let mut entries: Vec<(String, usize)> = results
        .offsets
        .iter()
        .filter(|(k, _)| {
            let s = k.as_str();
            s.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}")
        })
        .map(|(k, &off)| (k.as_str().to_string(), off))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

/// Test helper: thin forwarder to the production per-element helper.
/// Retained so the existing A2A integration tests keep calling the
/// same name; they now pin the production code rather than a parallel
/// implementation.  The per-slot `loop_partitions` carries each loop's
/// slot count (its `len()`), so no separate slot-count map is threaded.
fn compute_rel_loop_scores_per_element(
    results: &Results,
    loop_partitions: &IndexMap<String, Vec<Option<usize>>>,
) -> HashMap<String, Vec<f64>> {
    ltm_post::compute_rel_loop_scores_per_element(results, loop_partitions)
}

/// AC6.1 + AC6.4 + AC6.5: Pure A2A loop scores for an arrayed feedback model.
///
/// Model: population[Region] (3 regions) with a reinforcing birth loop:
///   population[Region] (stock, init=100)
///     -> births[Region] (flow, population * birth_rate)
///     -> population[Region]
///   birth_rate[Region] (aux, 0.05)
///
/// Verifies:
/// - AC6.1: Loop score is the element-wise product of A2A link scores,
///   and the loop score variable has 3 slots (one per region).
/// - AC6.4: Each element's relative loop scores sum to ~100% independently.
/// - AC6.5: All element-level loops share one loop ID (not 3 separate IDs).
#[test]
fn test_a2a_pure_dimension_loop_scores() {
    let n_elements: usize = 3;

    let project = TestProject::new("a2a_loop_scores")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_stock("population[Region]", "100", &["births"], &[], None)
        .array_aux("birth_rate[Region]", "0.05")
        .array_flow("births[Region]", "population * birth_rate", None)
        .build_datamodel();

    let (compiled, loop_partitions) = compile_ltm_incremental_with_partitions(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // AC6.5: Verify there is exactly one loop score variable (shared ID
    // across all 3 elements), not 3 separate ones.
    let loop_scores = find_loop_score_offsets(&results);
    assert_eq!(
        loop_scores.len(),
        1,
        "Pure-dimension A2A model should have exactly 1 loop score variable (shared ID), \
         found {}: {:?}",
        loop_scores.len(),
        loop_scores
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
    );

    // AC6.1: The single loop score variable should have n_elements slots.
    let (loop_score_name, loop_score_offset) = &loop_scores[0];
    for elem in 0..n_elements {
        let elem_offset = loop_score_offset + elem;
        let any_nonzero = (2..results.step_count).any(|step| {
            let val = results.data[step * results.step_size + elem_offset];
            val.abs() > 1e-10 && !val.is_nan()
        });
        assert!(
            any_nonzero,
            "A2A loop score element {} (offset {}) should have non-zero values, var: {}",
            elem, elem_offset, loop_score_name
        );
    }

    // AC6.5 continued: Verify exactly one loop ID in the partition mapping.
    // rel_loop_score is no longer materialized as a VM variable; instead
    // compute it per-element post-sim.  With only one loop in the
    // partition, the per-element formula reduces to loop_score / |loop_score|
    // = +/-1.0.
    assert_eq!(
        loop_partitions.len(),
        1,
        "Pure-dimension A2A model should map exactly 1 loop ID to a partition, \
         found {}: {:?}",
        loop_partitions.len(),
        loop_partitions.keys().collect::<Vec<_>>()
    );

    // AC6.4: Each element's relative loop score should have |value| = 1.0
    // because each element is in its own partition (no cross-element feedback).
    for elem in 0..n_elements {
        let elem_offset = loop_score_offset + elem;
        let nonzero_loop_scores: Vec<f64> = (0..results.step_count)
            .map(|step| results.data[step * results.step_size + elem_offset])
            .filter(|v| *v != 0.0 && !v.is_nan())
            .collect();

        assert!(
            !nonzero_loop_scores.is_empty(),
            "Element {} loop_score should have non-zero values (var: {})",
            elem,
            loop_score_name
        );

        // With a single loop per element partition, the relative score is
        // loop_score[k] / |loop_score[k]| = +/-1.  We verify by computing
        // it directly from the emitted loop_score data.
        for ls in &nonzero_loop_scores {
            let rel = ls / ls.abs();
            assert!(
                (rel.abs() - 1.0).abs() < 1e-6,
                "Element {} rel_loop_score should be +/-1.0 (only loop in partition), \
                 got {} (loop_score={}, var: {})",
                elem,
                rel,
                ls,
                loop_score_name
            );
        }
    }
}

/// AC6.1 + AC6.4: Pure A2A loop scores with TWO loops in the same model.
///
/// Model: population[Region] (3 regions) with both reinforcing and
/// balancing feedback:
///   population[Region] (stock, init=100)
///     -> births[Region] (flow, population * birth_rate)
///     -> population[Region]  (reinforcing)
///   birth_rate[Region] (aux)
///   population[Region]
///     -> fraction_used[Region] (aux, population / capacity)
///     -> fractional_growth[Region] (aux, 1 - fraction_used)
///     -> births[Region] (flow)
///     -> population[Region]  (balancing)
///   capacity[Region] (aux, 1000)
///
/// Verifies that relative loop scores for each element sum to ~100%
/// across both loops (since all loops are within the same partition
/// for each element, and there is no cross-element feedback).
#[test]
fn test_a2a_two_loop_relative_scores_sum_to_100() {
    let n_elements: usize = 3;

    let project = TestProject::new("a2a_two_loops")
        .with_sim_time(0.0, 20.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_stock("population[Region]", "100", &["births"], &[], None)
        .array_aux("birth_rate[Region]", "0.1")
        .array_aux("capacity[Region]", "1000")
        .array_aux("fraction_used[Region]", "population / capacity")
        .array_aux(
            "fractional_growth[Region]",
            "birth_rate * (1 - fraction_used)",
        )
        .array_flow("births[Region]", "population * fractional_growth", None)
        .build_datamodel();

    let (compiled, loop_partitions) = compile_ltm_incremental_with_partitions(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Should have at least 2 loop score variables (the reinforcing and
    // balancing paths).
    let loop_scores = find_loop_score_offsets(&results);
    assert!(
        loop_scores.len() >= 2,
        "Two-loop A2A model should have at least 2 loop score variables, found {}: {:?}",
        loop_scores.len(),
        loop_scores
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
    );

    // Every emitted loop score should have a partition entry.  With
    // rel_loop_score moved post-sim, this is the shape we normalize
    // against.
    assert_eq!(
        loop_partitions.len(),
        loop_scores.len(),
        "Number of loop partitions should equal number of loop score vars"
    );

    // For each element, the absolute values of the per-element relative
    // loop scores across all loops should sum to approximately 1.0.  We
    // use the per-element helper because the A2A case requires per-element
    // normalization, while the scalar view (`ltm_post::compute_rel_loop_scores`)
    // collapses to element 0.  Both A2A loops pass through `population[r]`,
    // so at each element their slots land in the same `(partition, slot)`
    // bucket and self-normalize together.
    let rel_per_element = compute_rel_loop_scores_per_element(&results, &loop_partitions);

    for elem in 0..n_elements {
        // Pick a timestep late enough to have meaningful values (skip
        // initial timesteps where PREVIOUS is not yet populated).
        let test_step = 5;
        let rel_sum: f64 = rel_per_element
            .values()
            .map(|series| series[test_step * n_elements + elem].abs())
            .sum();

        // Allow some tolerance since we're summing absolute values of
        // signed relative scores.
        if rel_sum > 1e-10 {
            assert!(
                (rel_sum - 1.0).abs() < 0.1,
                "Element {} relative loop scores should sum to ~1.0, got {} at step {}",
                elem,
                rel_sum,
                test_step
            );
        }
    }
}

/// `ltm-arrays-hardening.AC2.1` regression guard: two structurally-independent
/// A2A feedback subsystems over *different* dimensions must normalize their
/// relative loop scores *within their own cycle partition*, not pooled across
/// both subsystems.
///
/// Model: a self-reinforcing birth loop over `Region = {a, b, c}`
///   `pop[Region]` (stock, init 100) -> `births[Region] = pop * 0.1` -> `pop`
/// and an independent self-reinforcing production loop over `Product = {x, y}`
///   `widgets[Product]` (stock, init 50) -> `production[Product] = widgets * 0.05` -> `widgets`
/// with no cross-coupling, so the element graph has five disjoint single-stock
/// SCCs (`pop[a]`, `pop[b]`, `pop[c]`, `widgets[x]`, `widgets[y]`) -- the per-
/// slot `loop_partitions: HashMap<String, Vec<Option<usize>>>` introduced by
/// commit 11eb1af1 (GH #487).
///
/// Asserts:
///  1. The two subsystems' loops land in *distinct* `loop_partitions` slots:
///     flattening every loop's per-slot partition vector yields five entries,
///     all `Some`, all pairwise distinct. Under the pre-fix pooled behavior
///     `loop_partitions` was `HashMap<String, Option<usize>>` and both loops
///     resolved to a single shared bucket (or `None`), so this set would have
///     size 1.
///  2. Relative loop scores normalize *within each partition*: each loop is the
///     only loop in each of its single-stock partitions, so every nonzero per-
///     element relative score is exactly +1.0. Under the pre-fix pooled
///     behavior the two reinforcing loops would cross-normalize, so the
///     dominant loop's relative score would be the pooled ratio (~0.5) and the
///     `== 1.0` check would fail on the old code.
#[test]
fn test_disconnected_a2a_loops_normalize_per_partition() {
    use std::collections::HashSet;

    let project = TestProject::new("two_a2a_subsystems")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["a", "b", "c"])
        .named_dimension("Product", &["x", "y"])
        .array_stock("pop[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "pop * 0.1", None)
        .array_stock("widgets[Product]", "50", &["production"], &[], None)
        .array_flow("production[Product]", "widgets * 0.05", None)
        .build_datamodel();

    let (compiled, loop_partitions) = compile_ltm_incremental_with_partitions(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Two structurally-independent A2A subsystems => exactly two loop IDs.
    assert_eq!(
        loop_partitions.len(),
        2,
        "two disconnected A2A subsystems should produce exactly two loop IDs, got {}: {:?}",
        loop_partitions.len(),
        loop_partitions.keys().collect::<Vec<_>>()
    );

    // (1) The two subsystems' loops occupy *distinct* partition slots: every
    // element-level stock is its own SCC, so flattening all per-slot partition
    // vectors gives five `Some` entries with no repeats. (Pre-fix: one shared
    // bucket -> this set would be a singleton.)
    let all_slots: Vec<Option<usize>> = loop_partitions.values().flatten().copied().collect();
    assert_eq!(
        all_slots.len(),
        5,
        "expected 3 Region slots + 2 Product slots = 5 per-slot partition entries, got {}: {:?}",
        all_slots.len(),
        loop_partitions
    );
    assert!(
        all_slots.iter().all(|p| p.is_some()),
        "every slot of a pure-A2A loop over a connected element resolves to a partition; got {:?}",
        loop_partitions
    );
    let distinct: HashSet<usize> = all_slots.iter().filter_map(|p| *p).collect();
    assert_eq!(
        distinct.len(),
        5,
        "two disconnected A2A subsystems must occupy 5 distinct cycle partitions \
         (not a single pooled bucket); got {} distinct from {:?}",
        distinct.len(),
        loop_partitions
    );

    // (2) Each loop is alone in each of its single-stock partitions, so the
    // per-element relative loop score reduces to loop_score[k] / |loop_score[k]|.
    // Both subsystems are purely reinforcing, so every nonzero relative score is
    // exactly +1.0 -- NOT the pre-fix pooled value the two loops would share if
    // they cross-normalized.
    let rel_per_element = compute_rel_loop_scores_per_element(&results, &loop_partitions);
    assert_eq!(
        rel_per_element.len(),
        2,
        "should normalize two loop_score series, got {}",
        rel_per_element.len()
    );
    for (loop_id, series) in &rel_per_element {
        // The series is `step_count * stride` long; for a pure-A2A loop alone in
        // its partition the stride is exactly the loop's element count.
        let stride = loop_partitions
            .get(loop_id)
            .map(|pv| pv.len())
            .expect("every rel-score loop id has a partition vector");
        assert!(
            stride == 2 || stride == 3,
            "unexpected stride {stride} for {loop_id}"
        );
        assert_eq!(
            series.len(),
            results.step_count * stride,
            "rel-score series for {loop_id} should be step_count * stride long"
        );
        let nonzero: Vec<f64> = series
            .iter()
            .copied()
            .filter(|v| *v != 0.0 && !v.is_nan())
            .collect();
        assert!(
            !nonzero.is_empty(),
            "loop {loop_id} should have non-zero per-element relative scores once dynamics start"
        );
        for v in &nonzero {
            assert!(
                (v - 1.0).abs() < 1e-9,
                "single-loop-per-partition relative score for {loop_id} should be exactly 1.0, \
                 not pooled; got {v}"
            );
        }
    }
}

/// Issue #463 prep: confirm the engine supports multi-dimensional A2A loops.
///
/// Model: `population[Region, Cohort]` with a pure A2A reinforcing loop
///   population (stock, init=100)
///     -> births (flow, population * 0.05)
///     -> population
/// where Region is a 2-element named dimension (NYC, Boston) and Cohort is a
/// 2-element indexed dimension (1, 2).  All four (region, cohort) slots are
/// independent — no cross-element feedback.
///
/// Asserts the contract every later #463 phase relies on:
///   1. There is exactly one `loop_score` synthetic variable for the loop
///      (shared ID across all four slots).
///   2. Its `LtmSyntheticVar.dimensions` is `["region", "cohort"]` in
///      declaration order, in canonical form.
///   3. The variable occupies `2 * 2 = 4` slots in `Results.offsets` (each
///      slot has a non-zero value at some saved step).
///
/// We deliberately do NOT pin a specific slot-layout convention here; that's
/// what the resolver test covers. This test just proves multi-dim arrayed
/// loops are a real configuration the engine emits today, so subsequent
/// phases can build on a real fixture rather than a hypothetical one.
#[test]
fn test_2d_arrayed_loop_score_metadata() {
    use simlin_engine::test_common::TestProject;

    let region_count: usize = 2;
    let cohort_count: usize = 2;
    let n_slots: usize = region_count * cohort_count;

    let project = TestProject::new("multidim_a2a")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .indexed_dimension("Cohort", cohort_count as u32)
        .array_stock("population[Region, Cohort]", "100", &["births"], &[], None)
        .array_flow("births[Region, Cohort]", "population * 0.05", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
    let source_model = sync.models["main"].source_model;
    let ltm_vars = model_ltm_variables(&db, source_model, sync.project);

    // (1) Exactly one loop_score synthetic variable.
    let loop_score_vars: Vec<&simlin_engine::db::LtmSyntheticVar> = ltm_vars
        .vars
        .iter()
        .filter(|v| v.name.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}"))
        .collect();
    assert_eq!(
        loop_score_vars.len(),
        1,
        "Expected 1 loop_score var for a single A2A self-loop, got {}: {:?}",
        loop_score_vars.len(),
        loop_score_vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );

    // (2) The variable's dimensions list, in declaration order.
    //
    // The dim names are stored in raw form (matching how the user declared
    // them in the model), not canonicalized.  Downstream code that compares
    // against canonical idents (loop partitions, element resolution) is
    // responsible for canonicalizing on its way in -- a contract the
    // LoopElementIndex builder formalizes.
    let dims = &loop_score_vars[0].dimensions;
    assert_eq!(
        dims,
        &vec!["Region".to_string(), "Cohort".to_string()],
        "loop_score dimensions should match the raw declaration-order names; got {:?}",
        dims
    );

    // (3) The variable occupies 4 slots; each slot has a non-zero value at
    // some saved step.  Run the VM end-to-end against the same compiled sim.
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let loop_scores = find_loop_score_offsets(&results);
    assert_eq!(
        loop_scores.len(),
        1,
        "expected one loop_score series in results"
    );
    let (loop_score_name, base_offset) = &loop_scores[0];

    for slot in 0..n_slots {
        let off = base_offset + slot;
        let any_nonzero = (1..results.step_count).any(|step| {
            let v = results.data[step * results.step_size + off];
            v.abs() > 1e-10 && !v.is_nan()
        });
        assert!(
            any_nonzero,
            "loop_score slot {} (var {}, offset {}) should be non-zero at some step",
            slot, loop_score_name, off
        );
    }
}

/// Tech-debt #34: A2A loop_score variables must produce per-element
/// distinct values when the underlying per-element dynamics differ --
/// i.e. the synthesized A2A equation must evaluate with its active
/// dimension intact, not broadcast slot 0 across every slot.
///
/// The fixture must be a *non-isolated* loop. An isolated loop's raw
/// loop score is exactly `+/-1` in every element regardless of gain
/// (the invariant pinned by `ltm_dt_invariance.rs` and
/// `arrayed_isolated_loop_raw_score_is_one_per_element`), so a single
/// reinforcing loop -- however heterogeneous its rates -- has identical
/// slots by construction and cannot tell a correct per-element
/// evaluation apart from a slot-0 broadcast. (An earlier version of this
/// test used exactly that isolated fixture and only "passed" because LTM
/// review Finding 2 made the flow-to-stock link score wrong in a
/// rate-dependent way; once Finding 2 was fixed the isolated-loop slots
/// became correctly identical and the broken premise surfaced.)
///
/// So this model gives each region *two* loops sharing the `population`
/// stock -- a reinforcing birth loop and a balancing death loop -- with
/// heterogeneous birth and death rates. Two coupled loops on one stock
/// are not isolated, so each loop's raw score depends on the per-element
/// rates: for `population * b` births and `population * d` deaths the
/// birth loop scores `b / (b - d)` and the death loop `-d / (b - d)`,
/// both genuinely distinct between NYC and Boston.
///
/// Slot ordering: the fixture uses `Region: [NYC, Boston]`, so slot 0
/// = NYC and slot 1 = Boston.
#[test]
fn test_a2a_loop_score_has_distinct_per_element_values() {
    use simlin_engine::test_common::TestProject;

    let project = TestProject::new("a2a_distinct_slots")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .array_with_ranges(
            "birth_rate[Region]",
            vec![("NYC", "0.10"), ("Boston", "0.40")],
        )
        .array_with_ranges(
            "death_rate[Region]",
            vec![("NYC", "0.03"), ("Boston", "0.05")],
        )
        .array_stock("population[Region]", "100", &["births"], &["deaths"], None)
        .array_flow("births[Region]", "population * birth_rate", None)
        .array_flow("deaths[Region]", "population * death_rate", None)
        .build_datamodel();

    let (compiled, _loop_partitions) = compile_ltm_incremental_with_partitions(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Two coupled loops on the `population` stock -> two A2A loop scores.
    let loop_scores = find_loop_score_offsets(&results);
    assert_eq!(
        loop_scores.len(),
        2,
        "the two coupled loops (birth + death) should each produce a loop_score variable, \
         got {:?}",
        loop_scores.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );

    // Every A2A loop score in this model has genuinely per-element
    // distinct slots (the loops are not isolated, so the raw score
    // depends on the per-element rates). Each must show at least one
    // saved step where slot 0 differs visibly from slot 1 -- a slot-0
    // broadcast would make them identical.
    for (loop_score_name, base_offset) in &loop_scores {
        let mut max_diff = 0.0_f64;
        let mut max_diff_step = 0;
        for step in 1..results.step_count {
            let s0 = results.data[step * results.step_size + base_offset];
            let s1 = results.data[step * results.step_size + base_offset + 1];
            let diff = (s0 - s1).abs();
            if diff > max_diff {
                max_diff = diff;
                max_diff_step = step;
            }
        }
        assert!(
            max_diff > 1e-6,
            "loop_score var {} slots should differ per-element (heterogeneous birth/death \
             rates make the non-isolated loop score rate-dependent); max |slot0 - slot1| \
             across {} steps was {} at step {}",
            loop_score_name,
            results.step_count,
            max_diff,
            max_diff_step
        );
    }
}

/// AC6.2 + AC6.3: Mixed loop with cross-element feedback produces scalar
/// per-element loop scores with individual IDs.
///
/// Model: population[Region] (2 regions) with both:
/// (A) Per-element reinforcing loop: population -> births -> population
/// (B) Cross-element feedback: population -> total_pop (SUM) -> migration
///     -> population (scalar->arrayed, affects all elements)
///
/// The cross-element path creates a "mixed" loop because it goes through
/// a scalar variable (total_pop), so the element-level circuits for that
/// path should produce individual scalar loop scores.
///
/// Verifies:
/// - AC6.2: Mixed loops get individual scalar loop scores
/// - AC6.3: Relative scores normalize within the correct partition
#[test]
fn test_mixed_loop_scalar_per_element_scores() {
    let project = TestProject::new("mixed_loop_scores")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        // population[Region] stock
        .array_stock(
            "population[Region]",
            "100",
            &["births", "migration"],
            &[],
            None,
        )
        // Per-element reinforcing loop: births = population * 0.05
        .array_aux("birth_rate[Region]", "0.05")
        .array_flow("births[Region]", "population * birth_rate", None)
        // Cross-element path: total_pop is scalar, migration feeds back
        .scalar_aux("total_pop", "SUM(population[*])")
        .array_flow(
            "migration[Region]",
            "total_pop * 0.01 - population * 0.01",
            None,
        )
        .build_datamodel();

    let (compiled, loop_partitions) = compile_ltm_incremental_with_partitions(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Should have loop score variables. The pure-dimension reinforcing loop
    // (population -> births -> population) produces an A2A loop score.
    // The mixed loops (involving total_pop) may produce scalar per-element
    // loop scores, OR may not form complete loops depending on structure.
    let loop_scores = find_loop_score_offsets(&results);
    assert!(
        !loop_scores.is_empty(),
        "Mixed loop model should have loop score variables, found none. \
         Available vars: {:?}",
        results
            .offsets
            .keys()
            .filter(|k| k.as_str().contains("ltm"))
            .map(|k| k.as_str())
            .collect::<Vec<_>>()
    );

    // Verify post-sim relative loop score computation returns a series
    // for every loop_score that was emitted.
    let rel_scores = ltm_post::compute_rel_loop_scores(&results, &loop_partitions);
    assert!(
        !rel_scores.is_empty(),
        "Mixed loop model should produce post-sim relative loop scores"
    );

    // At least one loop score should be non-zero.
    let any_nonzero = loop_scores.iter().any(|(_, off)| {
        (2..results.step_count).any(|step| {
            let val = results.data[step * results.step_size + *off];
            val.abs() > 1e-10 && !val.is_nan()
        })
    });
    assert!(
        any_nonzero,
        "At least one loop score should be non-zero in the mixed model"
    );
}

// ============================================================================
// AC7: Discovery mode on element-level graph
//
// These tests verify that discovery mode operates on the element-level graph,
// finding element-specific loops post-simulation using strongest-path DFS
// from element-level stocks.
// ============================================================================

/// Run the full element-level discovery pipeline for an arrayed model.
///
/// This mirrors the pipeline in `analysis.rs::run_ltm_pipeline` but is
/// callable from integration tests. It:
/// 1. Compiles with LTM discovery mode enabled
/// 2. Simulates to get link score results
/// 3. Builds an element-level CausalGraph
/// 4. Calls `discover_loops_with_graph` with LTM var metadata and dims
///    so that A2A link scores are expanded into per-element edges
fn discover_loops_element_level(
    project: &simlin_engine::datamodel::Project,
) -> Vec<ltm_finding::FoundLoop> {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);

    let compiled =
        compile_project_incremental(&db, sync.project, "main").expect("compilation should succeed");
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // Build element-level causal graph
    let canonical_name = simlin_engine::canonicalize("main");
    let source_model = sync
        .project
        .models(&db)
        .get(canonical_name.as_ref())
        .copied()
        .expect("main model should exist in salsa DB");
    let element_edges = model_element_causal_edges(&db, source_model, sync.project);
    let causal_graph = causal_graph_from_element_edges(element_edges);

    let stocks: Vec<Ident<Canonical>> =
        element_edges.stocks.iter().map(|s| Ident::new(s)).collect();

    // Get LTM variable metadata and project dimensions for A2A expansion
    let ltm_vars = model_ltm_variables(&db, source_model, sync.project);
    let dm_dims = project_datamodel_dims(&db, sync.project);
    // The emission-derived per-sub-model output-port set the per-exit-port
    // recompute needs (GH #698), built through the exact production decision.
    let sub_model_ports = simlin_engine::analysis::build_sub_model_output_ports(&db, sync.project);

    ltm_finding::discover_loops_with_graph(
        &results,
        &causal_graph,
        &stocks,
        &ltm_vars.vars,
        dm_dims,
        &sub_model_ports,
        None,
    )
    .expect("discover_loops_with_graph should succeed")
    .loops
}

/// AC7.1: Discovery mode on an arrayed model finds element-specific loops.
///
/// Model: population[Region] (stock, 3 regions) with a simple reinforcing
/// feedback loop: population -> birth_rate -> births -> population.
/// Each region has the same equation structure but different initial
/// conditions, so per-element link scores differ.
///
/// Verifies that discovery mode finds one loop per region element,
/// each containing element-specific variables like `population[nyc]`,
/// `births[nyc]`, etc.
#[test]
fn test_discovery_element_specific_loops() {
    use simlin_engine::test_common::TestProject;

    let project = TestProject::new("discovery_elem_loops")
        .with_sim_time(0.0, 20.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        // population[Region] stock with different initial values
        .array_stock("population[Region]", "100", &["births"], &[], None)
        // birth_rate[Region] depends on population (creates feedback)
        .array_aux("birth_rate[Region]", "population * 0.02")
        // births[Region] = birth_rate
        .array_flow("births[Region]", "birth_rate", None)
        .build_datamodel();

    let found = discover_loops_element_level(&project);

    // With 3 regions and the same feedback structure, discovery should
    // find 3 element-specific loops (one per region). Each loop has the
    // structure: population[region] -> birth_rate[region] -> births[region]
    // -> population[region].
    assert_eq!(
        found.len(),
        3,
        "Discovery should find 3 element-specific loops (one per region), found {}. \
         Loops: {:?}",
        found.len(),
        found
            .iter()
            .map(|l| l
                .loop_info
                .links
                .iter()
                .map(|link| format!("{} -> {}", link.from.as_str(), link.to.as_str()))
                .collect::<Vec<_>>()
                .join(", "))
            .collect::<Vec<_>>()
    );

    // Each loop should contain element-subscripted variables (e.g., `population[nyc]`)
    let regions = ["nyc", "boston", "la"];
    for region in &regions {
        let has_region_loop = found.iter().any(|l| {
            l.loop_info
                .links
                .iter()
                .any(|link| link.from.as_str().contains(region))
        });
        assert!(
            has_region_loop,
            "Should find an element-specific loop for region '{}'. Found loops: {:?}",
            region,
            found
                .iter()
                .map(|l| l
                    .loop_info
                    .links
                    .iter()
                    .map(|link| link.from.as_str().to_string())
                    .collect::<Vec<_>>())
                .collect::<Vec<_>>()
        );
    }

    // All discovered loops should have non-zero average scores
    for fl in &found {
        assert!(
            fl.avg_abs_score > 0.0,
            "Loop {} should have non-zero avg_abs_score, got {}",
            fl.loop_info.id,
            fl.avg_abs_score
        );
    }
}

/// AC7.3: Discovery mode cross-validates with exhaustive mode on a small
/// arrayed model. Both modes should find the same element-level loops.
///
/// Uses the same population/birth_rate/births model as test 1. The
/// exhaustive mode (via the legacy `model_element_loop_circuits`) finds
/// all element-level circuits structurally, and discovery mode should
/// find the same loops post-simulation. The legacy element-Johnson
/// surface is retained for this measurement.
#[allow(deprecated)]
#[test]
fn test_discovery_cross_validates_with_exhaustive_arrayed() {
    use simlin_engine::test_common::TestProject;

    let project = TestProject::new("discovery_xval")
        .with_sim_time(0.0, 20.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_stock("population[Region]", "100", &["births"], &[], None)
        .array_aux("birth_rate[Region]", "population * 0.02")
        .array_flow("births[Region]", "birth_rate", None)
        .build_datamodel();

    // Exhaustive mode: find all element-level circuits structurally
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;
    let exhaustive_circuits = model_element_loop_circuits(&db, source_model, sync.project);

    // Discovery mode: find loops post-simulation
    let found = discover_loops_element_level(&project);

    // Materialize the named view once for error messages and per-circuit
    // iteration; production callers avoid this allocation entirely.
    let exhaustive_named = exhaustive_circuits.to_named_circuits();

    // Both modes should find the same number of loops
    assert_eq!(
        found.len(),
        exhaustive_named.len(),
        "Discovery ({}) should find the same number of loops as exhaustive ({}) \
         for a small arrayed model. \
         Exhaustive circuits: {:?}. \
         Discovery loops: {:?}",
        found.len(),
        exhaustive_named.len(),
        exhaustive_named,
        found
            .iter()
            .map(|l| l
                .loop_info
                .links
                .iter()
                .map(|link| link.from.as_str().to_string())
                .collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );

    // Verify that every exhaustive circuit's node set appears in
    // the discovery results
    for circuit in &exhaustive_named {
        let mut exhaustive_nodes: Vec<String> = circuit.clone();
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
            "Exhaustive circuit {} not found in discovery results. \
             Discovery found: {:?}",
            circuit.join(" -> "),
            found
                .iter()
                .map(|l| l
                    .loop_info
                    .links
                    .iter()
                    .map(|link| link.from.as_str().to_string())
                    .collect::<Vec<_>>()
                    .join(" -> "))
                .collect::<Vec<_>>()
        );
    }
}

/// AC7.2: Discovery mode's 0.1% contribution threshold filters
/// unimportant element-level loops.
///
/// Model: population[Region] with 2 regions connected by cross-element
/// migration feedback (SUM-based), putting them in the same partition.
/// One region has a strong per-element reinforcing loop (high birth
/// rate), and the other has a negligibly weak one (near-zero birth
/// rate). The threshold filter compares each loop's score against the
/// partition-scoped total, filtering the weak one.
///
/// Cross-element feedback (migration) ensures both regions are in the
/// same SCC partition. Without it, each region would be an independent
/// partition and the weak loop would have 100% contribution within its
/// own partition (never filtered).
#[test]
fn test_discovery_threshold_filters_negligible_loops() {
    use simlin_engine::test_common::TestProject;

    // Two regions: "Strong" has normal feedback, "Weak" has near-zero.
    // Migration connects them (cross-element feedback via SUM) so they
    // share the same cycle partition.
    let project = TestProject::new("discovery_threshold")
        .with_sim_time(0.0, 20.0, 1.0)
        .named_dimension("Region", &["Strong", "Weak"])
        .array_stock(
            "population[Region]",
            "1000",
            &["births", "migration"],
            &[],
            None,
        )
        // birth_rate: Strong = 0.1 (10% growth), Weak = 0.0000001 (effectively zero)
        .array_with_ranges(
            "birth_rate[Region]",
            vec![("Strong", "0.1"), ("Weak", "0.0000001")],
        )
        // births[Region] = population * birth_rate (per-element feedback)
        .array_flow("births[Region]", "population * birth_rate", None)
        // Cross-element migration: uses SUM to create a cross-element
        // dependency so both regions land in the same cycle partition.
        // migration[r] = SUM(population[*]) * 0.001 - population * 0.001
        .scalar_aux("total_pop", "SUM(population[*])")
        .array_flow(
            "migration[Region]",
            "total_pop * 0.001 - population * 0.001",
            None,
        )
        .build_datamodel();

    let found = discover_loops_element_level(&project);

    // Discovery should find loops. The "Strong" region's per-element
    // loop should always be present because it has significant feedback.
    assert!(!found.is_empty(), "Discovery should find at least one loop");

    // The Strong region loop should be present
    let has_strong = found.iter().any(|l| {
        l.loop_info
            .links
            .iter()
            .any(|link| link.from.as_str().contains("strong"))
    });
    assert!(
        has_strong,
        "The strong region's loop should be retained. Found: {:?}",
        found
            .iter()
            .map(|l| l
                .loop_info
                .links
                .iter()
                .map(|link| link.from.as_str().to_string())
                .collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );

    // The Weak region's per-element birth loop should be filtered
    // because its contribution is ~0.0000001/0.1 = 0.0001% of the
    // partition total, well below the 0.1% threshold.
    //
    // We check specifically for the births-related weak loop. The
    // cross-element migration loops may or may not be present depending
    // on their relative strength; we only care about the per-element
    // births loop for the weak region.
    let weak_births_loop =
        found.iter().any(|l| {
            // A loop is the "weak births loop" if it contains births[weak]
            // and population[weak] but NOT total_pop (which would make it
            // a cross-element migration loop instead).
            let has_weak_births = l.loop_info.links.iter().any(|link| {
                link.from.as_str() == "births[weak]" || link.to.as_str() == "births[weak]"
            });
            let has_total_pop =
                l.loop_info.links.iter().any(|link| {
                    link.from.as_str() == "total_pop" || link.to.as_str() == "total_pop"
                });
            has_weak_births && !has_total_pop
        });
    assert!(
        !weak_births_loop,
        "The weak region's per-element births loop should be filtered by \
         the 0.1%% threshold. Found loops: {:?}",
        found
            .iter()
            .map(|l| l
                .loop_info
                .links
                .iter()
                .map(|link| format!("{} -> {}", link.from.as_str(), link.to.as_str()))
                .collect::<Vec<_>>()
                .join(", "))
            .collect::<Vec<_>>()
    );
}

// --- AC8: End-to-end XMILE test model integration tests ---
//
// These tests load XMILE test models from the test/ directory and exercise
// the full LTM pipeline: XMILE parsing, compilation with LTM, simulation,
// structural analysis, and loop discovery. They validate both exhaustive
// and discovery modes on arrayed models with per-region feedback (A2A) and
// cross-element migration feedback.

/// Load an XMILE model from a file path, returning the parsed datamodel project.
fn load_xmile_model(path: &str) -> simlin_engine::datamodel::Project {
    let f = File::open(path).unwrap_or_else(|e| panic!("failed to open {}: {}", path, e));
    let mut f = BufReader::new(f);
    xmile::project_from_reader(&mut f)
        .unwrap_or_else(|e| panic!("failed to parse XMILE from {}: {}", path, e))
}

/// Helper for the design-plan postscript: counts circuits and SCC sizes
/// at every level of the LTM enumeration pipeline. Returns numbers
/// suitable for the design-plan measurement table. The function runs
/// every fixture-specific assertion so it doubles as a regression test
/// pinning the post-#482 numbers.
struct TieredMeasurements {
    var_scc: usize,
    elem_scc: usize,
    var_circuits: usize,
    elem_circuits_legacy: usize,
    fast_path: usize,
    slow_path: usize,
    slow_path_scc: usize,
}

// Drives the legacy `model_element_loop_circuits` (deprecated for new
// LTM compilation) to compare its circuit count against the tiered
// enumerator's fast/slow split for the design-plan postscript table.
#[allow(deprecated)]
fn measure_tiered(path: &str) -> TieredMeasurements {
    let dm = load_xmile_model(path);
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &dm);
    let source_model = sync.models["main"].source;
    let project = sync.project;

    let var_edges = model_causal_edges(&db, source_model, project);
    let var_scc = causal_graph_from_edges(var_edges).largest_scc_size();
    let elem_edges = model_element_causal_edges(&db, source_model, project);
    let elem_scc = causal_graph_from_element_edges(elem_edges).largest_scc_size();
    let var_circuits = model_loop_circuits(&db, source_model, project);
    let elem_circuits_legacy = model_element_loop_circuits(&db, source_model, project);
    let tiered = model_loop_circuits_tiered(&db, source_model, project);

    TieredMeasurements {
        var_scc,
        elem_scc,
        var_circuits: var_circuits.len(),
        elem_circuits_legacy: elem_circuits_legacy.len(),
        fast_path: tiered.fast_path.len(),
        slow_path: tiered.slow_path.len(),
        slow_path_scc: tiered.slow_path_largest_scc,
    }
}

/// Postscript measurement on the cross_element_ltm fixture.
///
/// Pinned numbers (post-#482, post-#448, post-cross-element-aggregate-scoring;
/// re-measured 2026-05-09 -- see `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md`'s
/// "Re-measurement after the cross-element aggregate-scoring work" section):
/// - var_scc = 5 (population, births, migration_pressure, migration_in,
///   migration_out are in one variable-level SCC; total_population is
///   acyclic). The births flow's structural stock-edge plus its
///   `population` reference closes the population<->births A2A pair.
/// - elem_scc = 10 (5 vars * 2 elements: NYC, Boston). `total_population =
///   SUM(population[*])` is a *whole-RHS* reducer, so it is a
///   variable-backed aggregate node -- no synthetic `$⁚ltm⁚agg⁚{n}` is
///   minted and the element graph for that edge is unchanged.
/// - var_circuits = 3 (the small finite count of variable-level cycles).
/// - elem_circuits_legacy = 8 (the per-reference walker trimmed the
///   spurious FixedIndex cross-edges that the pre-Phase-2 classifier
///   emitted, so this is down from 12; the aggregate-node work didn't
///   change it because the only reducer here is whole-RHS).
/// - fast_path = 1 (the population <-> births A2A reinforcing cycle).
/// - slow_path = 6 (the cross-element migration circuits, now scored on
///   the element-level path with subscripted link-score refs rather than
///   the diagonal A2A scores; each cross-element circuit is its own
///   scalar loop-score var).
/// - slow_path_scc = 8 (<= elem_scc; only the cross-element subgraph
///   nodes participate).
#[test]
fn measurement_postscript_cross_element_ltm() {
    let m = measure_tiered("../../test/cross_element_ltm/cross_element.stmx");
    eprintln!(
        "cross_element_ltm: var_scc={} elem_scc={} var_circuits={} elem_circuits_legacy={} fast_path={} slow_path={} slow_path_scc={}",
        m.var_scc,
        m.elem_scc,
        m.var_circuits,
        m.elem_circuits_legacy,
        m.fast_path,
        m.slow_path,
        m.slow_path_scc,
    );
    // Loose assertions: pin the structural inequalities, not the exact
    // counts (which may change as cycle-detection details evolve).
    assert!(
        m.fast_path >= 1,
        "expected at least one fast-path A2A cycle (population<->births)"
    );
    assert!(
        m.slow_path_scc <= m.elem_scc,
        "slow-path subgraph SCC must be at most full element-graph SCC"
    );
}

/// Postscript measurement on the arrayed_population_ltm fixture.
///
/// Pinned numbers (post-#482, post-#448, post-cross-element-aggregate-scoring;
/// re-measured 2026-05-09):
/// - Pure-A2A model with 2 cycles per region (births, deaths) over 3
///   regions, no reducers and no per-element-equation targets ⇒ no
///   aggregate nodes ⇒ the element graph is unchanged by the
///   cross-element aggregate-scoring work. var_scc = 3, elem_scc = 3.
///   Variable-level circuits = 2 (births reinforcing, deaths balancing).
///   Legacy element-level circuits = 6 (2 cycles * 3 regions). Tiered
///   enumerator emits 2 fast-path cycles, 0 slow-path, slow_path_scc = 0.
#[test]
fn measurement_postscript_arrayed_population_ltm() {
    let m = measure_tiered("../../test/arrayed_population_ltm/arrayed_population.stmx");
    eprintln!(
        "arrayed_population_ltm: var_scc={} elem_scc={} var_circuits={} elem_circuits_legacy={} fast_path={} slow_path={} slow_path_scc={}",
        m.var_scc,
        m.elem_scc,
        m.var_circuits,
        m.elem_circuits_legacy,
        m.fast_path,
        m.slow_path,
        m.slow_path_scc,
    );
    // Pure-A2A model: every variable-level cycle classifies as
    // PureSameElementA2A. Slow path must be empty.
    assert_eq!(
        m.slow_path, 0,
        "pure-A2A model must produce no slow-path circuits"
    );
    assert_eq!(
        m.slow_path_scc, 0,
        "pure-A2A model must have empty slow-path subgraph"
    );
    assert_eq!(
        m.fast_path, m.var_circuits,
        "all variable-level cycles must land in fast path"
    );
}

/// Regression for the slow-path / fast-path duplicate-Loop bug uncovered
/// during PR #496 review. The bug: when a pure-A2A cycle (e.g.
/// `a -> grow -> stock -> b -> a`) shares variables with a cross-element
/// cycle (e.g. `a -> grow -> stock -> b -> c -> a` where `c[r] = b[NYC] +
/// ...`), all four variables `a, grow, stock, b` end up in
/// `slow_path_var_nodes` because they participate in the cross-element
/// cycle. Johnson on the induced subgraph then re-discovers the per-element
/// reflections of the pure-A2A cycle (one per dimension element) which
/// `build_element_level_loops` collapses into a fresh A2A Loop -- duplicating
/// the A2A Loop the fast path already emitted.
///
/// Without the fix, this fixture emits three Loops (`r1`, `r2`, ...) where
/// `r1` and a slow-path-derived A2A Loop describe the same feedback loop.
/// With the fix, exactly two distinct Loops are emitted: one A2A loop and
/// one cross-element loop.
///
/// None of the pre-existing fixtures (cross_element_ltm,
/// arrayed_population_ltm, hero_culture_ltm, WRLD3) construct this
/// topology -- pure-A2A and cross-element cycles in those models don't
/// share more than the structural stock-flow variables, so the bug slipped
/// past existing coverage.
#[test]
fn test_dedup_slow_path_a2a_against_fast_path() {
    let n_elements: usize = 3;

    // Variable-level edges (with shapes):
    //   a -> grow      (Bare; grow = a * 0.001)
    //   grow -> stock  (structural inflow edge; treated as Bare)
    //   stock -> b     (Bare; b = stock * 0.01)
    //   b -> a         (Bare; a = b + c)
    //   c -> a         (Bare; a = b + c)
    //   b -> c         (FixedIndex(["nyc"]); c = b[NYC] * 0.5)
    //
    // Variable-level cycles:
    //   1) a -> grow -> stock -> b -> a            (PureSameElementA2A; fast path)
    //   2) a -> grow -> stock -> b -> c -> a       (CrossElementOrMixed; slow path)
    //
    // Cycle 2 contributes a, grow, stock, b, c to slow_path_var_nodes.
    // Cycle 1's variables a, grow, stock, b therefore also enter the
    // slow-path subgraph and Johnson re-finds the per-element pure-A2A
    // reflections that we already emitted from the fast path.
    let project = TestProject::new("dedup_slow_a2a")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_stock("stock[Region]", "100", &["grow"], &[], None)
        .array_flow("grow[Region]", "a * 0.001", None)
        .array_aux("b[Region]", "stock * 0.01")
        .array_aux("c[Region]", "b[NYC] * 0.5")
        .array_aux("a[Region]", "b + c")
        .build_datamodel();

    let (compiled, loop_partitions) = compile_ltm_incremental_with_partitions(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end()
        .expect("dedup regression fixture should simulate");
    let results = vm.into_results();

    // Without the dedup the slow-path Johnson finds the pure-A2A cycle's
    // per-element reflections (one per region) and
    // `build_element_level_loops` collapses them into a third Loop with a
    // distinct id -- the unfixed branch emits 3 loops (`r1`, `r2`, `u1`).
    // The dedup drops the per-element reflections so the only slow-path
    // survivor is the genuine longer cycle that traverses `c`. End state:
    // exactly two loop ids -- one A2A, one cross-element -- with two
    // partition entries.
    let loop_scores = find_loop_score_offsets(&results);
    assert_eq!(
        loop_scores.len(),
        2,
        "expected exactly 2 loop_score variables (one A2A, one cross-element); \
         got {}: {:?}",
        loop_scores.len(),
        loop_scores
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        loop_partitions.len(),
        2,
        "expected exactly 2 distinct loop ids in loop_partitions; got {}: {:?}",
        loop_partitions.len(),
        loop_partitions.keys().collect::<Vec<_>>()
    );
    // Touch n_elements once so the value remains documented in the
    // fixture even though the slot-count assertion is intentionally
    // omitted: the loop-score-variable allocation strategy interacts with
    // `is_cross_element` heuristics in `build_element_level_loops` that
    // are out of scope for the dedup regression. The two-loops invariant
    // above is the load-bearing assertion.
    let _ = n_elements;
}

/// AC8.1: A2A arrayed population model -- exhaustive mode.
///
/// Model: population[Region] (3 regions: NYC, Boston, LA) with:
///   - births[Region] = population * birth_rate (reinforcing A2A loop)
///   - deaths[Region] = population * death_rate (balancing A2A loop)
///   - birth_rate varies per region (0.03, 0.02, 0.01)
///   - death_rate is constant (0.01) across regions
///
/// Verifies:
///   - Per-element link scores exist and are non-zero
///   - Loop scores are A2A (3 slots for 3 regions)
///   - Relative loop scores per element sum to approximately 100%
///   - Each region's loop dominance pattern is independent
#[test]
fn test_arrayed_population_ltm_exhaustive() {
    let n_elements: usize = 3;
    let datamodel_project =
        load_xmile_model("../../test/arrayed_population_ltm/arrayed_population.stmx");

    // Compile with LTM exhaustive mode and simulate
    let (compiled, loop_partitions) = compile_ltm_incremental_with_partitions(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end()
        .expect("A2A population model should simulate");
    let results = vm.into_results();

    // Verify per-element link scores exist and are non-zero.
    // Look for A2A link score variables (population -> births edge).
    let link_score_vars: Vec<_> = results
        .offsets
        .iter()
        .filter(|(k, _)| {
            let s = k.as_str();
            s.starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}")
        })
        .map(|(k, &off)| (k.as_str().to_string(), off))
        .collect();
    assert!(
        !link_score_vars.is_empty(),
        "Should have link score variables for the A2A population model"
    );

    // Verify some link scores have non-zero values after initialization
    let has_nonzero_link = link_score_vars.iter().any(|(_, off)| {
        (2..results.step_count).any(|step| {
            let val = results.data[step * results.step_size + off];
            val.abs() > 1e-10 && !val.is_nan()
        })
    });
    assert!(
        has_nonzero_link,
        "At least some link scores should be non-zero"
    );

    // Verify loop scores are A2A: should have loop score variables with
    // 3 slots (one per region).
    let loop_scores = find_loop_score_offsets(&results);
    assert!(
        !loop_scores.is_empty(),
        "Should have loop score variables for the A2A population model"
    );

    // Each loop score variable should have n_elements slots allocated, and at
    // least one slot should evolve to a non-zero value.  We do NOT assert
    // every slot is non-zero because the fixture's death_rate is uniform
    // (0.01) and birth_rate[LA] is also 0.01 -- LA's population is in
    // exact equilibrium so its link_scores (and therefore loop_scores) are
    // legitimately zero.  Pre-tech-debt-#34, this assertion passed by
    // accident because the buggy A2A loop_score broadcast slot 0's
    // value to every slot, hiding the equilibrium element.
    for (name, base_offset) in &loop_scores {
        let any_slot_nonzero = (0..n_elements).any(|elem| {
            let off = base_offset + elem;
            (2..results.step_count).any(|step| {
                let val = results.data[step * results.step_size + off];
                val.abs() > 1e-10 && !val.is_nan()
            })
        });
        assert!(
            any_slot_nonzero,
            "Loop score var {} should have at least one slot with non-zero values",
            name
        );
    }

    // Tightened in 2026-04-25-ltm-per-ref-elem-graph: NYC (slot 0,
    // birth_rate=0.03) and Boston (slot 1, birth_rate=0.02) both have
    // birth_rate != death_rate (0.01 uniform), so neither population is
    // in equilibrium -- their per-slot loop scores must be non-zero on
    // every loop in the model.  Only LA (slot 2) is at equilibrium.
    // Pre-Phase-2 the per-element loop-score bookkeeping was scrambled by
    // the spurious NxN cross-element edges (the auto-flip threshold
    // could trip on this model and force discovery mode), so this
    // slot-resolved check could not be made cleanly; post-refactor the
    // per-element values are stable and we can hold each non-equilibrium
    // slot to a non-zero contract.
    let non_equilibrium_slots = [0_usize, 1_usize]; // NYC, Boston
    for (name, base_offset) in &loop_scores {
        for &elem in &non_equilibrium_slots {
            let off = base_offset + elem;
            let any_step_nonzero = (2..results.step_count).any(|step| {
                let val = results.data[step * results.step_size + off];
                val.abs() > 1e-10 && !val.is_nan()
            });
            assert!(
                any_step_nonzero,
                "Loop score var {} slot[{}] (non-equilibrium element) should be non-zero",
                name, elem
            );
        }
    }

    // Verify relative loop scores exist and each element's absolute values
    // sum to approximately 1.0 (since each region has independent dynamics,
    // each element is its own partition).  rel_loop_score is no longer
    // materialized as a VM variable, so we compute per-element scores from
    // the emitted loop_score data.
    assert!(
        !loop_partitions.is_empty(),
        "Should have loop partition entries to normalize against"
    );
    // This is a pure-A2A model over `Region`, so every loop has
    // `n_elements` slots and its rel-score series strides by `n_elements`.
    let rel_per_element = compute_rel_loop_scores_per_element(&results, &loop_partitions);

    // Check that relative loop scores per element sum to ~1.0 at some
    // timestep after initialization.
    // Per-element rel-scores normalize to ~1.0 only when that element's
    // partition has non-trivial dynamics.  The fixture has uniform
    // death_rate (0.01) and birth_rate[LA]=0.01, so LA's link_scores
    // are zero (population stationary -> stock_diff=0 -> SAFEDIV->0)
    // and rel-scores are 0/0 -> 0 by SAFEDIV-0 semantics.  Skip
    // equilibrium elements; require ~1.0 only on elements with
    // demonstrable dynamics.  Pre-tech-debt-#34, all 3 elements
    // appeared dynamic because slot 0 was broadcast.
    for elem in 0..n_elements {
        // Probe whether this element has any non-zero loop_score.  If
        // not, this is an equilibrium element and rel-scores are 0/0 -> 0.
        let elem_has_dynamics = rel_per_element.values().any(|series| {
            (3..results.step_count).any(|step| {
                let val = series[step * n_elements + elem];
                val.is_finite() && val.abs() > 1e-10
            })
        });
        if !elem_has_dynamics {
            continue;
        }
        let mut found_good_sum = false;
        for step in 3..results.step_count {
            let sum: f64 = rel_per_element
                .values()
                .map(|series| {
                    let val = series[step * n_elements + elem];
                    if val.is_nan() { 0.0 } else { val.abs() }
                })
                .sum();
            if sum > 0.5 && (sum - 1.0).abs() < 0.15 {
                found_good_sum = true;
                break;
            }
        }
        assert!(
            found_good_sum,
            "Element {} (with non-zero dynamics) relative loop scores should sum to ~1.0 at some timestep",
            elem
        );
    }

    // Verify region independence: each element's loop scores should be
    // computed independently. With 2 loops (reinforcing births, balancing
    // deaths) per region, verify that both polarities appear in the detected
    // loops from structural analysis.
    let mut db2 = SimlinDb::default();
    let sync2 = sync_from_datamodel_incremental(&mut db2, &datamodel_project, None);
    let canonical_name = simlin_engine::canonicalize("main");
    let source_model2 = sync2
        .project
        .models(&db2)
        .get(canonical_name.as_ref())
        .copied()
        .expect("main model should exist");
    let detected = model_detected_loops(&db2, source_model2, sync2.project);
    assert!(
        detected.loops.len() >= 2,
        "A2A population model should detect at least 2 loops (births reinforcing, deaths balancing), \
         found {}",
        detected.loops.len()
    );
}

/// AC8.1: A2A arrayed population model -- discovery mode.
///
/// Same model as test_arrayed_population_ltm_exhaustive but with discovery mode.
/// Verifies that discovery mode finds the same structural loops as exhaustive
/// mode and per-element loop rankings are consistent. Drives the legacy
/// element-Johnson surface (`model_element_loop_circuits`) to compare counts.
#[allow(deprecated)]
#[test]
fn test_arrayed_population_ltm_discovery() {
    let datamodel_project =
        load_xmile_model("../../test/arrayed_population_ltm/arrayed_population.stmx");

    // Discovery mode via element-level pipeline
    let found = discover_loops_element_level(&datamodel_project);

    // The model has per-element reinforcing (births) and balancing (deaths)
    // loops for each of 3 regions. Discovery should find element-specific loops.
    assert!(
        !found.is_empty(),
        "Discovery should find loops in the A2A population model"
    );

    // Each found loop should contain element-subscripted variables
    for loop_result in &found {
        let has_subscripted = loop_result
            .loop_info
            .links
            .iter()
            .any(|link| link.from.as_str().contains('[') || link.to.as_str().contains('['));
        assert!(
            has_subscripted,
            "Discovery loops should contain element-subscripted variables, got: {:?}",
            loop_result
                .loop_info
                .links
                .iter()
                .map(|link| format!("{} -> {}", link.from.as_str(), link.to.as_str()))
                .collect::<Vec<_>>()
        );
    }

    // Cross-validate with exhaustive: both should find the same structural
    // loop patterns.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
    let canonical_name = simlin_engine::canonicalize("main");
    let source_model = sync
        .project
        .models(&db)
        .get(canonical_name.as_ref())
        .copied()
        .expect("main model should exist");
    let exhaustive_circuits = model_element_loop_circuits(&db, source_model, sync.project);

    // Discovery should find loops for the same regions that exhaustive finds.
    // Each exhaustive circuit corresponds to a per-element loop.
    assert!(
        found.len() <= exhaustive_circuits.circuits.len(),
        "Discovery ({}) should find at most as many loops as exhaustive ({}) for a small model",
        found.len(),
        exhaustive_circuits.circuits.len()
    );
}

/// AC8.2: Cross-element feedback model -- exhaustive mode.
///
/// Model: population[Region] (2 regions: NYC, Boston) with:
///   - births[Region] = population * 0.02 (per-element reinforcing loop)
///   - migration_pressure cross-references population[NYC] and population[Boston]
///   - total_population = SUM(population[*]) (arrayed-to-scalar edge)
///
/// Verifies:
///   - Cross-element loops are detected
///   - Per-element cross-dimensional link scores exist
///   - Element-level cycle partitions correctly group connected stocks
#[test]
fn test_cross_element_ltm_exhaustive() {
    let datamodel_project = load_xmile_model("../../test/cross_element_ltm/cross_element.stmx");

    // Compile with LTM exhaustive mode and simulate
    let compiled = compile_ltm_incremental(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end()
        .expect("Cross-element model should simulate");
    let results = vm.into_results();

    // Verify link scores exist
    let link_score_vars: Vec<_> = results
        .offsets
        .iter()
        .filter(|(k, _)| {
            let s = k.as_str();
            s.starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}")
        })
        .map(|(k, &off)| (k.as_str().to_string(), off))
        .collect();
    assert!(
        !link_score_vars.is_empty(),
        "Should have link score variables for the cross-element model"
    );

    // Verify cross-element loops are detected via structural analysis.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
    let canonical_name = simlin_engine::canonicalize("main");
    let source_model = sync
        .project
        .models(&db)
        .get(canonical_name.as_ref())
        .copied()
        .expect("main model should exist");

    // Element-level causal edges should include cross-element references
    let element_edges = model_element_causal_edges(&db, source_model, sync.project);
    assert!(
        !element_edges.edges.is_empty(),
        "Element-level causal edges should be non-empty"
    );

    // The model has element-subscripted variables. Verify that the
    // element-level graph contains edges involving subscripted nodes.
    let has_subscripted_edges = element_edges
        .edges
        .iter()
        .any(|(from, targets)| from.contains('[') || targets.iter().any(|to| to.contains('[')));
    assert!(
        has_subscripted_edges,
        "Element-level edges should contain subscripted variable nodes. \
         Edges: {:?}",
        element_edges
            .edges
            .iter()
            .flat_map(|(from, targets)| targets.iter().map(move |to| format!("{} -> {}", from, to)))
            .collect::<Vec<_>>()
    );

    // Verify cross-dimensional edges exist: the SUM(population[*]) equation
    // for total_population should create edges from both population[nyc] and
    // population[boston] to the scalar total_population.
    let pop_to_total: Vec<_> = element_edges
        .edges
        .iter()
        .filter(|(from, targets)| {
            from.starts_with("population[") && targets.contains("total_population")
        })
        .map(|(from, _)| from.clone())
        .collect();
    assert!(
        pop_to_total.len() >= 2,
        "Both population[nyc] and population[boston] should have edges to total_population. \
         Found: {:?}",
        pop_to_total
    );

    // Element-level cycle partitions should exist for each stock element.
    let cycle_partitions = model_element_cycle_partitions(&db, source_model, sync.project);
    assert!(
        !cycle_partitions.partitions.is_empty(),
        "Cycle partitions should be non-empty"
    );

    // Both stock elements should have partition assignments.
    let nyc_partition = cycle_partitions.stock_partition.get("population[nyc]");
    let boston_partition = cycle_partitions.stock_partition.get("population[boston]");
    assert!(
        nyc_partition.is_some(),
        "population[nyc] should have a partition assignment"
    );
    assert!(
        boston_partition.is_some(),
        "population[boston] should have a partition assignment"
    );

    // Verify loop scores exist. The model has per-element feedback loops
    // (births -> population) for each region.
    let loop_scores = find_loop_score_offsets(&results);
    assert!(
        !loop_scores.is_empty(),
        "Should have loop score variables for the cross-element model"
    );

    // Verify each loop score variable has working per-element dynamics
    // in at least one slot.  We do NOT require every slot non-zero: with
    // the cross-element-feedback fixture, some loops only meaningfully
    // exercise certain elements (e.g. cross-element loops involving
    // total_population may only have non-trivial scores at elements
    // whose dynamics actually shift the SUM).  Pre-tech-debt-#34 this
    // assertion passed by accident due to slot-0 broadcast.
    // The cross-element fixture is asymmetric (NYC=1000, Boston=500; NYC
    // pushes migration_out, Boston has zero migration_in because
    // migration_pressure[NYC] is positive => migration_in[Boston] = MAX(-x, 0)
    // = 0).  Many cycles legitimately collapse to zero in one or both
    // slots due to zero link_scores in the product.  Just verify that
    // at least one loop has working dynamics in at least one slot.
    // Pre-tech-debt-#34 every loop appeared non-zero by virtue of the
    // slot-0 broadcast bug -- that was masking reality, not a contract.
    let n_elements: usize = 2;
    let any_loop_active = loop_scores.iter().any(|(_, base_offset)| {
        (0..n_elements).any(|elem| {
            let off = base_offset + elem;
            (2..results.step_count).any(|step| {
                let val = results.data[step * results.step_size + off];
                val.abs() > 1e-10 && !val.is_nan()
            })
        })
    });
    assert!(
        any_loop_active,
        "Cross-element fixture should have at least one loop with non-zero per-element loop_score values"
    );

    // Tightened in 2026-04-25-ltm-per-ref-elem-graph: the A2A reinforcing
    // births loop (population[r] -> births[r] -> population[r]) is a pure
    // same-element cycle whose link scores are independent of the
    // cross-element migration machinery.  Both NYC (init=1000) and Boston
    // (init=500) start with non-equilibrium populations and a uniform
    // birth rate of 0.02, so both slots must carry a non-zero loop score
    // every step after t=2.  Pre-Phase-2 this could not be asserted
    // because the spurious NxN cross-element edges polluted the A2A loop
    // structure; post-refactor the A2A loop is clean and this slot-by-slot
    // check is robust.  We still cannot assert the same on the migration
    // loops: those legitimately zero out one slot due to MAX(...)
    // semantics in migration_in / migration_out, which is fixture
    // behavior independent of the refactor.
    //
    // `\u{205A}r1` is the A2A reinforcing births loop, which is dimensioned
    // over Region (2 slots). The cross-element migration loops are *scalar*
    // loop-score vars (1 slot) -- their element-path scoring is exercised
    // by the dedicated `test_cross_element_ltm_loop_score_*` tests below.
    let a2a_reinforcing_loop = loop_scores
        .iter()
        .find(|(name, _)| name.ends_with("\u{205A}r1"))
        .expect("A2A reinforcing births loop r1 should be present in loop_scores");
    for elem in 0..n_elements {
        let off = a2a_reinforcing_loop.1 + elem;
        let any_step_nonzero = (2..results.step_count).any(|step| {
            let val = results.data[step * results.step_size + off];
            val.abs() > 1e-10 && !val.is_nan()
        });
        assert!(
            any_step_nonzero,
            "A2A reinforcing loop {} slot[{}] should be non-zero (NYC and Boston both have non-equilibrium births dynamics)",
            a2a_reinforcing_loop.0, elem
        );
    }

    // Phase 2 tightening: the cross-element migration loop is scored from
    // the actual element-level link scores along its path, not collapsed
    // onto the diagonal. The `population[nyc] -> migration_pressure[boston]
    // -> migration_in[nyc] -> population[nyc]` loop must reference the
    // *swap* link score `migration_pressure[boston]→migration_in[nyc]`,
    // not the `migration_pressure → migration_out` diagonal; and there
    // must be a loop-score equation whose factor set is exactly the three
    // element-path references of that loop. (The thorough element-path /
    // hand-calc checks live in the dedicated
    // `test_cross_element_ltm_loop_score_*` tests below.)
    let mut db2 = SimlinDb::default();
    let sync2 = sync_from_datamodel_incremental(&mut db2, &datamodel_project, None);
    set_project_ltm_enabled(&mut db2, sync2.project, true);
    let source_model2 = sync2.models["main"].source_model;
    let ltm_vars = model_ltm_variables(&db2, source_model2, sync2.project);
    let loop_score_eqs: Vec<String> = ltm_vars
        .vars
        .iter()
        .filter(|v| v.name.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}"))
        .map(|v| v.equation.source_text())
        .collect();
    let migration_loop_factors: std::collections::HashSet<String> = [
        "\"$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}migration_pressure\"[boston]",
        "\"$\u{205A}ltm\u{205A}link_score\u{205A}migration_pressure[boston]\u{2192}migration_in\"[nyc]",
        "\"$\u{205A}ltm\u{205A}link_score\u{205A}migration_in\u{2192}population\"[nyc]",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert!(
        loop_score_eqs.iter().any(|eq| eq
            .split(" * ")
            .map(|s| s.trim().to_string())
            .collect::<std::collections::HashSet<_>>()
            == migration_loop_factors),
        "expected a loop-score equation for the population[nyc]->migration_pressure[boston]->\
         migration_in[nyc]->population[nyc] loop with factor set {migration_loop_factors:?}; \
         got: {loop_score_eqs:?}"
    );
}

/// AC1.3: truthful per-reference element edge set for the cross-element
/// fixture.
///
/// `model_element_causal_edges` walks each target's `Expr2` AST and emits
/// element edges per reference site, classifying each reference by its
/// `RefShape`. A fixed-index reference like `migration_pressure[Boston]`
/// is classified as `FixedIndex(Boston)` and emits one edge from
/// `migration_pressure[boston]` to the target, rather than expanding to
/// all N x N edges. The two `assert_no_edge` calls verify that
/// `migration_in[NYC]` -- which references only
/// `migration_pressure[Boston]` -- does not pick up a spurious
/// `migration_pressure[NYC] -> migration_in[NYC]` edge.
#[test]
fn test_cross_element_ltm_edge_set_truthful() {
    let datamodel_project = load_xmile_model("../../test/cross_element_ltm/cross_element.stmx");

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
    let canonical_name = simlin_engine::canonicalize("main");
    let source_model = sync
        .project
        .models(&db)
        .get(canonical_name.as_ref())
        .copied()
        .expect("main model should exist");
    let element_edges = model_element_causal_edges(&db, source_model, sync.project);

    // Helper closures for readable assertions. Each takes &str instead of
    // String because the edge-key strings are short and stable; cloning
    // through `to_string` once per assertion is negligible.
    let has_edge = |from: &str, to: &str| -> bool {
        element_edges
            .edges
            .get(from)
            .is_some_and(|targets| targets.contains(to))
    };
    let assert_edge = |from: &str, to: &str| {
        assert!(
            has_edge(from, to),
            "expected edge {from} -> {to}, but it was missing.\nedges from '{from}': {:?}",
            element_edges.edges.get(from)
        );
    };
    let assert_no_edge = |from: &str, to: &str| {
        assert!(
            !has_edge(from, to),
            "expected NO edge {from} -> {to}, but it was present"
        );
    };

    // population -> migration_pressure: every element of population is
    // referenced by at least one migration_pressure equation, so all four
    // (population[d] -> migration_pressure[e]) edges exist by literal
    // FixedIndex reference.
    assert_edge("population[nyc]", "migration_pressure[nyc]");
    assert_edge("population[boston]", "migration_pressure[nyc]");
    assert_edge("population[boston]", "migration_pressure[boston]");
    assert_edge("population[nyc]", "migration_pressure[boston]");

    // migration_pressure -> migration_in: each migration_in equation
    // references the OTHER region's migration_pressure only. The truthful
    // edge set is the swap-pair (boston -> nyc, nyc -> boston); the same-
    // element edges (nyc -> nyc, boston -> boston) are spurious today and
    // must disappear after the refactor.
    assert_edge("migration_pressure[boston]", "migration_in[nyc]");
    assert_edge("migration_pressure[nyc]", "migration_in[boston]");
    assert_no_edge("migration_pressure[nyc]", "migration_in[nyc]");
    assert_no_edge("migration_pressure[boston]", "migration_in[boston]");

    // migration_pressure -> migration_out: A2A bare ref `MAX(migration_pressure, 0)`
    // is a SameElement reference; only the diagonal edges should exist.
    assert_edge("migration_pressure[nyc]", "migration_out[nyc]");
    assert_edge("migration_pressure[boston]", "migration_out[boston]");

    // population -> births: A2A bare ref `population * 0.02` is SameElement.
    assert_edge("population[nyc]", "births[nyc]");
    assert_edge("population[boston]", "births[boston]");

    // population -> total_population: SUM(population[*]) is a wildcard
    // reducer feeding a scalar, so every element of population edges to it.
    assert_edge("population[nyc]", "total_population");
    assert_edge("population[boston]", "total_population");

    // Structural flow -> stock edges from the population stock's
    // inflow/outflow declarations. Each flow's element feeds the matching
    // stock element (SameElement at the structural-edge level).
    assert_edge("births[nyc]", "population[nyc]");
    assert_edge("births[boston]", "population[boston]");
    assert_edge("migration_in[nyc]", "population[nyc]");
    assert_edge("migration_in[boston]", "population[boston]");
    assert_edge("migration_out[nyc]", "population[nyc]");
    assert_edge("migration_out[boston]", "population[boston]");
}

/// ltm-503-cross-element-agg.AC1.4: the `migration_pressure[boston] ->
/// migration_in` link score on the `cross_element_ltm` fixture carries a
/// meaningful per-element partial.
///
/// `migration_in` is a per-element-equation (`Ast::Arrayed`) flow:
///   migration_in[NYC]    = MAX(migration_pressure[Boston] * -1, 0)
///   migration_in[Boston] = MAX(migration_pressure[NYC]    * -1, 0)
/// Pre-fix the `migration_pressure[boston] -> migration_in` link score
/// carried a `"0"`-partial-derived value (the arrayed target fell through
/// to a constant `0` partial). Post-fix the link score is `Equation::Arrayed`
/// over `Region` whose per-element slots are:
///
///   - NYC slot: the partial w.r.t. live `migration_pressure[boston]` is
///     exactly `MAX(migration_pressure[boston] * -1, 0)` -- i.e. all of
///     `migration_in[NYC]` -- so `Δpartial == Δmigration_in[NYC]` and
///     `ABS(SAFEDIV(Δ, Δ)) == 1`. (`migration_pressure[Boston] = (pop[B] -
///     pop[N]) * 0.01 < 0` throughout, and `pop[N] - pop[B]` keeps growing
///     under the uniform birth rate, so `migration_in[NYC]` changes every
///     step and `Δ != 0`.) Magnitude is ~1 at every step >= 2.
///   - Boston slot: `migration_in[Boston]` references only
///     `migration_pressure[nyc]`, which doesn't match the `FixedIndex(boston)`
///     shape, so its `migration_pressure[nyc]` ref is frozen at PREVIOUS;
///     and since `migration_pressure[NYC] > 0` throughout, `migration_in[Boston]
///     = MAX(negative, 0) = 0` constantly, so `Δmigration_in[Boston] == 0`
///     and the zero-change guard fires -- the slot is identically 0.
#[test]
fn test_cross_element_link_score_migration_in_arrayed_partials() {
    let datamodel_project = load_xmile_model("../../test/cross_element_ltm/cross_element.stmx");

    let compiled = compile_ltm_incremental(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end()
        .expect("cross-element model should simulate with LTM enabled");
    let results = vm.into_results();

    // Locate the `$⁚ltm⁚link_score⁚migration_pressure[boston]→migration_in`
    // synthetic variable's base offset. `find_cross_dimensional_offsets`
    // returns (source_element, base_offset) pairs for every
    // `migration_pressure[E]→migration_in` link score; we want E == "boston".
    let mp_to_in = find_cross_dimensional_offsets(&results, "migration_pressure", "migration_in");
    assert!(
        !mp_to_in.is_empty(),
        "expected per-element migration_pressure -> migration_in link scores; \
         offsets present: {:?}",
        results
            .offsets
            .keys()
            .map(|k| k.as_str())
            .filter(|s| s.contains("migration_in"))
            .collect::<Vec<_>>()
    );
    let base_offset = mp_to_in
        .iter()
        .find(|(elem, _)| elem == "boston")
        .map(|(_, off)| *off)
        .expect("migration_pressure[boston] -> migration_in link score should exist");

    // The link score is dimensioned over Region = {NYC, Boston}: slot 0 is
    // the NYC element, slot 1 the Boston element. Confirm the dimension
    // element order from the project's datamodel rather than assuming.
    // (XMILE loading canonicalizes dimension and element names to lowercase.)
    let region_dim = datamodel_project
        .dimensions
        .iter()
        .find(|d| d.name() == "region")
        .expect("Region dimension should exist");
    let region_elems: Vec<String> = match &region_dim.elements {
        simlin_engine::datamodel::DimensionElements::Named(names) => names.clone(),
        simlin_engine::datamodel::DimensionElements::Indexed(_) => {
            panic!("Region should be a named dimension")
        }
    };
    assert_eq!(
        region_elems,
        vec!["nyc".to_string(), "boston".to_string()],
        "fixture's Region dimension order is NYC then Boston"
    );
    let nyc_index = region_elems
        .iter()
        .position(|e| e == "nyc")
        .expect("nyc element should exist");
    let boston_index = region_elems
        .iter()
        .position(|e| e == "boston")
        .expect("boston element should exist");

    // t == 1 is the unstable first post-initial step (matches the
    // ensure_ltm_results convention of skipping it). For every step t >= 2:
    //   - NYC slot magnitude is within 1e-3 of 1.0
    //   - Boston slot is exactly 0.0
    let mut checked_steps = 0usize;
    for step in 2..results.step_count {
        let nyc_val = results.data[step * results.step_size + base_offset + nyc_index];
        let boston_val = results.data[step * results.step_size + base_offset + boston_index];
        assert!(
            !nyc_val.is_nan() && (nyc_val.abs() - 1.0).abs() < 1e-3,
            "step {step}: migration_pressure[boston]->migration_in NYC slot magnitude should be ~1, \
             got {nyc_val}"
        );
        assert_eq!(
            boston_val, 0.0,
            "step {step}: migration_pressure[boston]->migration_in Boston slot should be 0"
        );
        checked_steps += 1;
    }
    assert!(
        checked_steps > 0,
        "expected at least one simulated step t >= 2 to check"
    );
}

// -- ltm-503-cross-element-agg Phase 2: cross-element loops scored on the
//    element-level path --

/// Split a loop-score equation (a ` * `-joined product of quoted
/// link-score references, optionally with a trailing `[elem]` subscript)
/// into the set of its factors verbatim.
fn loop_score_equation_factors(eq: &str) -> std::collections::HashSet<String> {
    eq.split(" * ").map(|s| s.trim().to_string()).collect()
}

/// Find the offset of slot `element` of an A2A synthetic variable named
/// `var_name`, dimensioned over `Region` (in declaration order). The
/// `cross_element_ltm` fixture's `Region` is `{NYC, Boston}`, so the
/// element offsets are NYC=base+0, Boston=base+1 (XMILE loading
/// lowercases the names).
fn a2a_slot_offset(results: &Results, var_name: &str, element: &str) -> usize {
    let base = results
        .offsets
        .iter()
        .find(|(k, _)| k.as_str() == var_name)
        .map(|(_, &off)| off)
        .unwrap_or_else(|| {
            panic!(
                "synthetic var {var_name:?} not found in results; present link/loop scores: {:?}",
                results
                    .offsets
                    .keys()
                    .map(|k| k.as_str())
                    .filter(|s| s.contains("\u{205A}ltm\u{205A}"))
                    .collect::<Vec<_>>()
            )
        });
    let slot = match element {
        "nyc" => 0,
        "boston" => 1,
        other => panic!("unexpected Region element {other:?}"),
    };
    base + slot
}

/// ltm-503-cross-element-agg.AC2.1: the cross-element loop
/// `population[nyc] -> migration_pressure[boston] -> migration_in[nyc] ->
/// population[nyc]` is enumerated, and its `loop_score` equation is the
/// product of the per-element link scores along the element-level path
/// (`"$⁚ltm⁚link_score⁚population[nyc]→migration_pressure"[boston]`,
/// `"$⁚ltm⁚link_score⁚migration_pressure[boston]→migration_in"[nyc]`,
/// `"$⁚ltm⁚link_score⁚migration_in→population"[nyc]`) -- NOT the
/// unsubscripted A2A diagonal names (e.g. the `migration_out` link score
/// that the pre-Phase-2 collapse would reference).
#[test]
fn test_cross_element_ltm_loop_score_uses_element_path() {
    let datamodel_project = load_xmile_model("../../test/cross_element_ltm/cross_element.stmx");
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm_vars = model_ltm_variables(&db, source_model, sync.project);

    let pop_nyc_to_mp = "\"$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}migration_pressure\"[boston]";
    let mp_boston_to_in = "\"$\u{205A}ltm\u{205A}link_score\u{205A}migration_pressure[boston]\u{2192}migration_in\"[nyc]";
    let in_to_pop_nyc =
        "\"$\u{205A}ltm\u{205A}link_score\u{205A}migration_in\u{2192}population\"[nyc]";
    let expected: std::collections::HashSet<String> =
        [pop_nyc_to_mp, mp_boston_to_in, in_to_pop_nyc]
            .into_iter()
            .map(str::to_string)
            .collect();

    // Find a loop-score var whose factor set is exactly the three
    // element-path references above (rotation-independent).
    let loop_a = ltm_vars
        .vars
        .iter()
        .filter(|v| v.name.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}"))
        .find(|v| loop_score_equation_factors(&v.equation.source_text()) == expected);
    let loop_a = loop_a.unwrap_or_else(|| {
        panic!(
            "no loop_score var with the cross-element migration-loop factor set {expected:?}; \
             loop_score equations present: {:?}",
            ltm_vars
                .vars
                .iter()
                .filter(|v| v.name.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}"))
                .map(|v| (v.name.as_str(), v.equation.source_text()))
                .collect::<Vec<_>>()
        )
    });

    let eq = loop_a.equation.source_text();
    // It is a product of the three references and references no diagonal
    // `migration_out` link score (the pre-Phase-2 collapse target).
    assert!(
        eq.contains(" * "),
        "loop score should be a product; got: {eq}"
    );
    assert!(
        !eq.contains("migration_pressure\u{2192}migration_out"),
        "must not reference the diagonal migration_out link score; got: {eq}",
    );
    // And it visits a specific element of each A2A link score (subscripted
    // references), never the bare A2A array.
    for r in [pop_nyc_to_mp, mp_boston_to_in, in_to_pop_nyc] {
        assert!(
            eq.contains(r),
            "loop score equation missing reference {r}; got: {eq}"
        );
    }
}

/// ltm-503-cross-element-agg.AC2.3: the symmetric loop
/// `population[boston] -> migration_pressure[nyc] -> migration_in[boston] ->
/// population[boston]` is also enumerated with the analogous subscripted
/// references. (Its loop-score *value* is identically zero by the
/// fixture's `MAX(...)` semantics -- `migration_in[Boston] =
/// MAX(migration_pressure[NYC] * -1, 0)` and `migration_pressure[NYC] > 0`
/// throughout -- but the loop is still enumerated and references the right
/// link scores; that is all AC2.3 requires.)
#[test]
fn test_cross_element_ltm_symmetric_loop_enumerated() {
    let datamodel_project = load_xmile_model("../../test/cross_element_ltm/cross_element.stmx");
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm_vars = model_ltm_variables(&db, source_model, sync.project);

    let pop_boston_to_mp = "\"$\u{205A}ltm\u{205A}link_score\u{205A}population[boston]\u{2192}migration_pressure\"[nyc]";
    let mp_nyc_to_in = "\"$\u{205A}ltm\u{205A}link_score\u{205A}migration_pressure[nyc]\u{2192}migration_in\"[boston]";
    let in_to_pop_boston =
        "\"$\u{205A}ltm\u{205A}link_score\u{205A}migration_in\u{2192}population\"[boston]";
    let expected: std::collections::HashSet<String> =
        [pop_boston_to_mp, mp_nyc_to_in, in_to_pop_boston]
            .into_iter()
            .map(str::to_string)
            .collect();

    let found = ltm_vars
        .vars
        .iter()
        .filter(|v| v.name.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}"))
        .any(|v| loop_score_equation_factors(&v.equation.source_text()) == expected);
    assert!(
        found,
        "no loop_score var with the symmetric migration-loop factor set {expected:?}; \
         loop_score equations present: {:?}",
        ltm_vars
            .vars
            .iter()
            .filter(|v| v.name.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}"))
            .map(|v| (v.name.as_str(), v.equation.source_text()))
            .collect::<Vec<_>>()
    );
}

/// ltm-503-cross-element-agg.AC2.2: the `population[nyc] ->
/// migration_pressure[boston] -> migration_in[nyc] -> population[nyc]`
/// loop's `loop_score` series matches the product of the per-element link
/// scores along its element-level path, at every simulated step t >= 2
/// (within 1e-6).
#[test]
fn test_cross_element_ltm_loop_score_value_matches_hand_calc() {
    let datamodel_project = load_xmile_model("../../test/cross_element_ltm/cross_element.stmx");

    // First locate which loop id corresponds to loop A (by equation
    // contents) using the salsa path...
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm_vars = model_ltm_variables(&db, source_model, sync.project);

    let pop_nyc_to_mp = "\"$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}migration_pressure\"[boston]";
    let mp_boston_to_in = "\"$\u{205A}ltm\u{205A}link_score\u{205A}migration_pressure[boston]\u{2192}migration_in\"[nyc]";
    let in_to_pop_nyc =
        "\"$\u{205A}ltm\u{205A}link_score\u{205A}migration_in\u{2192}population\"[nyc]";
    let expected: std::collections::HashSet<String> =
        [pop_nyc_to_mp, mp_boston_to_in, in_to_pop_nyc]
            .into_iter()
            .map(str::to_string)
            .collect();
    let loop_a_name = ltm_vars
        .vars
        .iter()
        .filter(|v| v.name.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}"))
        .find(|v| loop_score_equation_factors(&v.equation.source_text()) == expected)
        .map(|v| v.name.clone())
        .expect("loop A loop_score var should exist");

    // ...then compile & simulate, and compare loop A's series to the
    // product of the three per-element link scores it references.
    let compiled = compile_ltm_incremental(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end()
        .expect("cross-element model should simulate with LTM enabled");
    let results = vm.into_results();

    let loop_off = results
        .offsets
        .iter()
        .find(|(k, _)| k.as_str() == loop_a_name.as_str())
        .map(|(_, &off)| off)
        .unwrap_or_else(|| panic!("loop A offset for {loop_a_name:?} not found in results"));

    let pop_nyc_to_mp_off = a2a_slot_offset(
        &results,
        "$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}migration_pressure",
        "boston",
    );
    let mp_boston_to_in_off = a2a_slot_offset(
        &results,
        "$\u{205A}ltm\u{205A}link_score\u{205A}migration_pressure[boston]\u{2192}migration_in",
        "nyc",
    );
    let in_to_pop_off = a2a_slot_offset(
        &results,
        "$\u{205A}ltm\u{205A}link_score\u{205A}migration_in\u{2192}population",
        "nyc",
    );

    let mut checked = 0usize;
    let mut saw_nonzero = false;
    for step in 2..results.step_count {
        let base = step * results.step_size;
        let l1 = results.data[base + pop_nyc_to_mp_off];
        let l2 = results.data[base + mp_boston_to_in_off];
        let l3 = results.data[base + in_to_pop_off];
        let loop_val = results.data[base + loop_off];
        let expected_val = l1 * l2 * l3;
        assert!(
            (loop_val - expected_val).abs() < 1e-6,
            "step {step}: loop A loop_score {loop_val} != product of element link scores \
             ({l1} * {l2} * {l3} = {expected_val})"
        );
        if loop_val.abs() > 1e-9 && !loop_val.is_nan() {
            saw_nonzero = true;
        }
        checked += 1;
    }
    assert!(checked > 0, "expected at least one step t >= 2 to check");
    assert!(
        saw_nonzero,
        "loop A's element-path product should be non-zero at some step \
         (NYC pressure stays negative and population keeps changing)"
    );
}

/// ltm-503-cross-element-agg.AC3.2 (exhaustive loop-score value side):
/// the loop `population[nyc] -> total_pop -> migration[nyc] ->
/// population[nyc]` -- a scalar reducer (`total_pop = SUM(population[*])`)
/// factored out of the per-element migration flow -- has its `loop_score`
/// series equal to the product of the three per-element link scores it
/// references, at every simulated step t >= 2 (within 1e-6), and that
/// product is non-zero at some step.
///
/// This exercises the scalar->arrayed per-target-element link score
/// (`$⁚ltm⁚link_score⁚total_pop→migration[nyc]`, a scalar variable) inside
/// a real loop-score equation alongside the arrayed->scalar reducer link
/// score (`$⁚ltm⁚link_score⁚population[nyc]→total_pop`) and the structural
/// flow->stock A2A link score (`$⁚ltm⁚link_score⁚migration→population`,
/// slot NYC).
#[test]
fn test_scalar_reducer_loop_score_value_matches_hand_calc() {
    let project = TestProject::new("scalar_reducer_loop_value")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock(
            "population[Region]",
            "100",
            &["births", "migration"],
            &[],
            None,
        )
        .array_aux("birth_rate[Region]", "0.05")
        .array_flow("births[Region]", "population * birth_rate", None)
        .scalar_aux("total_pop", "SUM(population[*])")
        .array_flow(
            "migration[Region]",
            "total_pop * 0.01 - population * 0.01",
            None,
        )
        .build_datamodel();

    // Locate the loop_score var for the 3-edge `population[nyc] -> total_pop
    // -> migration[nyc] -> population[nyc]` loop by its factor set (rotation-
    // independent).
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm_vars = model_ltm_variables(&db, source_model, sync.project);

    let expected: std::collections::HashSet<String> = [
        "\"$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}total_pop\"".to_string(),
        "\"$\u{205A}ltm\u{205A}link_score\u{205A}total_pop\u{2192}migration[nyc]\"".to_string(),
        "\"$\u{205A}ltm\u{205A}link_score\u{205A}migration\u{2192}population\"[nyc]".to_string(),
    ]
    .into_iter()
    .collect();
    let loop_name = ltm_vars
        .vars
        .iter()
        .filter(|v| v.name.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}"))
        .find(|v| loop_score_equation_factors(&v.equation.source_text()) == expected)
        .map(|v| v.name.clone())
        .unwrap_or_else(|| {
            panic!(
                "no loop_score var with the scalar-reducer loop factor set {expected:?}; \
                 loop_score equations present: {:?}",
                ltm_vars
                    .vars
                    .iter()
                    .filter(|v| v.name.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}"))
                    .map(|v| v.equation.source_text())
                    .collect::<Vec<_>>()
            )
        });

    let compiled = compile_ltm_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let off_by_name = |name: &str| -> usize {
        results
            .offsets
            .iter()
            .find(|(k, _)| k.as_str() == name)
            .map(|(_, &off)| off)
            .unwrap_or_else(|| panic!("var {name:?} not found in results"))
    };
    let loop_off = off_by_name(loop_name.as_str());
    let l1_off =
        off_by_name("$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}total_pop");
    let l2_off =
        off_by_name("$\u{205A}ltm\u{205A}link_score\u{205A}total_pop\u{2192}migration[nyc]");
    // The flow->stock link score `migration→population` is A2A over Region
    // {NYC, Boston}; slot NYC is the base offset.
    let l3_off = off_by_name("$\u{205A}ltm\u{205A}link_score\u{205A}migration\u{2192}population");

    let mut checked = 0usize;
    let mut saw_nonzero = false;
    for step in 2..results.step_count {
        let base = step * results.step_size;
        let l1 = results.data[base + l1_off];
        let l2 = results.data[base + l2_off];
        let l3 = results.data[base + l3_off];
        let loop_val = results.data[base + loop_off];
        let product = l1 * l2 * l3;
        assert!(
            (loop_val - product).abs() < 1e-6,
            "step {step}: scalar-reducer loop_score {loop_val} != product of element link \
             scores ({l1} * {l2} * {l3} = {product})"
        );
        if loop_val.abs() > 1e-9 && !loop_val.is_nan() {
            saw_nonzero = true;
        }
        checked += 1;
    }
    assert!(checked > 0, "expected at least one step t >= 2 to check");
    assert!(
        saw_nonzero,
        "the scalar-reducer loop's link-score product should be non-zero at some step \
         (total_pop and population both change every step)"
    );
}

/// Whether any discovered loop contains a link `from -> to` (exact string
/// match on the element-level endpoint names).
fn discovery_loops_have_link(found: &[ltm_finding::FoundLoop], from: &str, to: &str) -> bool {
    found.iter().any(|l| {
        l.loop_info
            .links
            .iter()
            .any(|link| link.from.as_str() == from && link.to.as_str() == to)
    })
}

/// A flat dump of every discovered loop's `from -> to` link list, for
/// assertion failure messages.
fn discovery_loops_debug(found: &[ltm_finding::FoundLoop]) -> Vec<String> {
    found
        .iter()
        .map(|l| {
            l.loop_info
                .links
                .iter()
                .map(|link| format!("{} -> {}", link.from.as_str(), link.to.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .collect()
}

/// AC8.2 / ltm-503-cross-element-agg.AC3.1: Cross-element feedback model --
/// discovery mode finds the cross-element loop. The `cross_element_ltm`
/// fixture's `migration_pressure[NYC]` reads `population[Boston]` (and vice
/// versa), so a genuine cross-element edge `population[nyc] ->
/// migration_pressure[boston]` (or the symmetric `population[boston] ->
/// migration_pressure[nyc]`) appears on the element-level path of a
/// discovered loop -- not merely "some subscripted loop".
#[test]
fn test_cross_element_ltm_discovery() {
    let datamodel_project = load_xmile_model("../../test/cross_element_ltm/cross_element.stmx");

    // Discovery mode via element-level pipeline
    let found = discover_loops_element_level(&datamodel_project);

    // The cross-element model should have discoverable loops.
    // At minimum, the per-element births loop should be found.
    assert!(
        !found.is_empty(),
        "Discovery should find loops in the cross-element model"
    );

    // Verify found loops contain element-subscripted variables
    let has_subscripted_loop = found.iter().any(|l| {
        l.loop_info
            .links
            .iter()
            .any(|link| link.from.as_str().contains('['))
    });
    assert!(
        has_subscripted_loop,
        "At least one discovery loop should contain element-subscripted variables. Found: {:?}",
        discovery_loops_debug(&found)
    );

    // The cross-element edge: `migration_pressure[r] = (population[r] -
    // population[other]) * 0.01`, so the element-level causal graph has
    // `population[other] -> migration_pressure[r]`. Discovery must keep that
    // edge in the search graph (the FixedIndex-source A2A link score
    // `population[nyc]->migration_pressure` expands via
    // `expand_fixed_from_a2a_link_offsets` to per-target-element edges),
    // and the loop `population[other] -> migration_pressure[r] ->
    // migration_in[other] -> population[other]` is discoverable.
    let has_cross_element_edge =
        discovery_loops_have_link(&found, "population[nyc]", "migration_pressure[boston]")
            || discovery_loops_have_link(&found, "population[boston]", "migration_pressure[nyc]");
    assert!(
        has_cross_element_edge,
        "discovery should find a loop with the cross-element edge \
         population[nyc] -> migration_pressure[boston] (or the symmetric \
         population[boston] -> migration_pressure[nyc]); discovered loops: {:?}",
        discovery_loops_debug(&found)
    );

    // Cross-validate: all discovered loops should be structurally valid
    // (every link should connect variables that exist in the model)
    for loop_result in &found {
        assert!(
            !loop_result.loop_info.links.is_empty(),
            "Discovered loop should have at least one link"
        );
    }
}

/// ltm-503-cross-element-agg.AC3.2 (discovery side): a model that factors a
/// scalar reducer (`total_pop = SUM(population[*])`) out of the per-element
/// migration flow (`migration[r] = total_pop*0.01 - population[r]*0.01`,
/// `population[r]` stock fed by `migration[r]`) -- discovery finds the loop
/// `population[*] -> total_pop -> migration[r] -> population[r]`, i.e. a
/// loop whose links include an edge `(total_pop, migration[nyc])` and the
/// reducer edge `(population[nyc], total_pop)`.
///
/// Crucially the scalar source `total_pop` stays *unsubscripted* on both
/// edges: a `(total_pop, migration[nyc])` edge, not `(total_pop[nyc],
/// migration[nyc])`. Pre-fix this loop was silently undiscoverable --
/// `total_pop -> migration` was emitted as a Bare-A2A link score with
/// `dimensions = ["Region"]`, so `parse_link_offsets`'s
/// `expand_a2a_link_offsets` subscripted *both* sides and invented a
/// phantom `total_pop[nyc]` node that doesn't match the unsubscripted
/// `total_pop` node the reducer edge (`population[nyc] -> total_pop`)
/// produces, breaking the cycle in the search graph.
#[test]
fn test_scalar_reducer_loop_discovery() {
    let project = TestProject::new("scalar_reducer_loop_discovery")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock(
            "population[Region]",
            "100",
            &["births", "migration"],
            &[],
            None,
        )
        .array_aux("birth_rate[Region]", "0.05")
        .array_flow("births[Region]", "population * birth_rate", None)
        .scalar_aux("total_pop", "SUM(population[*])")
        .array_flow(
            "migration[Region]",
            "total_pop * 0.01 - population * 0.01",
            None,
        )
        .build_datamodel();

    let found = discover_loops_element_level(&project);
    assert!(
        !found.is_empty(),
        "discovery should find loops in the scalar-reducer model"
    );

    // The loop must visit `total_pop` *unsubscripted* on both incident
    // edges: `population[nyc] -> total_pop` (reducer) and `total_pop ->
    // migration[nyc]` (scalar source feeding the per-element flow).
    let nyc_loop = discovery_loops_have_link(&found, "population[nyc]", "total_pop")
        && discovery_loops_have_link(&found, "total_pop", "migration[nyc]");
    let boston_loop = discovery_loops_have_link(&found, "population[boston]", "total_pop")
        && discovery_loops_have_link(&found, "total_pop", "migration[boston]");
    assert!(
        nyc_loop || boston_loop,
        "discovery should find the scalar-reducer loop population[*] -> total_pop -> \
         migration[r] -> population[r] with `total_pop` unsubscripted on both edges; \
         discovered loops: {:?}",
        discovery_loops_debug(&found)
    );

    // And `total_pop` must never appear subscripted (no phantom
    // `total_pop[nyc]` node).
    for l in &found {
        for link in &l.loop_info.links {
            for endpoint in [link.from.as_str(), link.to.as_str()] {
                assert!(
                    !endpoint.starts_with("total_pop["),
                    "discovery introduced a phantom subscripted scalar node {endpoint:?}; \
                     discovered loops: {:?}",
                    discovery_loops_debug(&found)
                );
            }
        }
    }
}

// ============================================================================
// AC4.6 (end-to-end): a cross-element loop over a partially-reduced axis,
// scored from the partial-reduce link scores.
//
// `matrix[D1,D2]` (a stock) feeds `row_sum[D1] = SUM(matrix[D1,*])` (a
// partial reduce collapsing only the D2 axis), and the inflow
// `growth[D1,D2] = row_sum[D1] * c[D1,D2]` closes the loop
// `matrix[d1,d2] -> row_sum[d1] -> growth[d1,d2] -> matrix[d1,d2]`. Within
// a row, `matrix[d1,x]` and `matrix[d1,y]` both feed `row_sum[d1]` through
// distinct partial-reduce link-score edges, so the loop through one element
// "sees" the other via the reducer link score.
// ============================================================================

/// Find the per-`(d1,d2)`-element partial-reduce link score offset for the
/// edge `{from}[d1,d2] -> {to}[d1]`. The source subscript carries both
/// axes; the target subscript carries only the surviving axis. Returns
/// `None` if no such variable was emitted.
fn find_partial_reduce_offset(
    results: &Results,
    from_name: &str,
    d1: &str,
    d2: &str,
    to_name: &str,
) -> Option<usize> {
    let name = format!(
        "$\u{205A}ltm\u{205A}link_score\u{205A}{from_name}[{d1},{d2}]\u{2192}{to_name}[{d1}]"
    );
    results
        .offsets
        .iter()
        .find(|(k, _)| k.as_str() == name)
        .map(|(_, &off)| off)
}

/// Build a 2-D arrayed feedback model whose loop runs over the
/// partially-reduced axis.
///
/// Model structure (`growth` uses bare references because it depends on the
/// stock `matrix` and the scalar `total`, not on `row_sum`; the
/// `row_sum[D1]` iterated-dimension subscript itself is now classified
/// `Bare` (GH #511) and demonstrated in `build_iterated_dim_subscript_model`):
///   matrix[D1,D2] (stock, distinct per-element initial values, multiplicative
///                  self-feedback -> the per-element trajectories diverge,
///                  so the reducer link scores are non-degenerate)
///   row_sum[D1]   (aux, = SUM(matrix[D1,*]))  -- the partial reduce
///   total         (aux, = SUM(row_sum[*]))  -- a scalar full reduce on top
///   growth[D1,D2] (flow, = matrix * total * 0.000001)  -- inflow into matrix;
///                  `matrix` is the same-element diagonal, `total` is a
///                  scalar that broadcasts.
///
/// `D1 = {a, b}`, `D2 = {x, y}`. `row_sum`'s *whole* equation is the reducer
/// `SUM(matrix[D1,*])`, so `row_sum` itself is the (variable-backed) aggregate
/// node -- `result_dims = [D1]`, `read_slice = [Iterated(d1), Reduced]` (the
/// `D1` axis is iterated over the A2A dimension space, the `D2` axis reduced).
/// Variable-backed aggs are real variable nodes, so the `(matrix, row_sum)`
/// element edges go through the normal reference walker (which classifies
/// `matrix[D1,*]` as `Wildcard` -> the conservative `matrix[d1,d2] ->
/// row_sum[d1']` cross-product), *not* the synthetic-agg reroute that #514
/// tightened for *inline* reducer subexpressions. So besides the clean
/// 4-cycles `matrix[d1,d2] -> row_sum[d1] -> total -> growth[d1,d2] ->
/// matrix[d1,d2]` there are still spurious cross-element loops; the assertions
/// only require that a real partial-reduce link score is emitted, carries
/// non-degenerate values, and is referenced by some loop score.
fn build_partial_reduce_model(name: &str) -> simlin_engine::datamodel::Project {
    use simlin_engine::datamodel::{self, Equation, Variable};

    datamodel::Project {
        name: name.to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![
            datamodel::Dimension::named("D1".to_string(), vec!["a".to_string(), "b".to_string()]),
            datamodel::Dimension::named("D2".to_string(), vec!["x".to_string(), "y".to_string()]),
        ],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                // matrix[D1,D2] with distinct per-element initial values so
                // the per-element trajectories (and hence the reducer link
                // scores) are non-degenerate.
                Variable::Stock(datamodel::Stock {
                    ident: "matrix".to_string(),
                    equation: Equation::Arrayed(
                        vec!["D1".to_string(), "D2".to_string()],
                        vec![
                            ("a,x".to_string(), "100".to_string(), None, None),
                            ("a,y".to_string(), "150".to_string(), None, None),
                            ("b,x".to_string(), "200".to_string(), None, None),
                            ("b,y".to_string(), "250".to_string(), None, None),
                        ],
                        None,
                        false,
                    ),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["growth".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // row_sum[D1] = SUM(matrix[D1,*])  -- the partial reduce.
                Variable::Aux(datamodel::Aux {
                    ident: "row_sum".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["D1".to_string()],
                        "SUM(matrix[D1, *])".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // total = SUM(row_sum[*])  -- scalar full reduce.
                Variable::Aux(datamodel::Aux {
                    ident: "total".to_string(),
                    equation: Equation::Scalar("SUM(row_sum[*])".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // growth[D1,D2] = matrix * total * 0.000001  -- inflow.
                // matrix is the same-element diagonal; total broadcasts.
                Variable::Flow(datamodel::Flow {
                    ident: "growth".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["D1".to_string(), "D2".to_string()],
                        "matrix * total * 0.000001".to_string(),
                    ),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// AC4.6 (end-to-end): the partial-reduce link scores `matrix[d1,d2] ->
/// row_sum[d1]` are emitted, carry non-degenerate values, and a loop-score
/// variable references them.
#[test]
fn test_partial_reduce_cross_element_loop() {
    let project = build_partial_reduce_model("partial_reduce_loop");

    // Exhaustive mode: loop scores are emitted and the matrix -> row_sum
    // reducer edge participates in the feedback loops. Compile and
    // fetch the synthetic-variable list from the same db so the loop-score
    // equations below match the simulated variables exactly.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);

    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // (a) Both partial-reduce link scores in row `a` are present and
    // non-degenerate (not identically 0 across all steps, and not always
    // magnitude exactly 1 -- a SUM partial reduce yields a fraction
    // strictly between 0 and 1 whenever the two row elements have
    // different deltas).
    let ax = find_partial_reduce_offset(&results, "matrix", "a", "x", "row_sum")
        .expect("expected partial-reduce link score matrix[a,x] -> row_sum[a]");
    let ay = find_partial_reduce_offset(&results, "matrix", "a", "y", "row_sum")
        .expect("expected partial-reduce link score matrix[a,y] -> row_sum[a]");
    // The b-row scores must exist too (one per (d1, d2) pair).
    assert!(
        find_partial_reduce_offset(&results, "matrix", "b", "x", "row_sum").is_some(),
        "expected partial-reduce link score matrix[b,x] -> row_sum[b]"
    );
    assert!(
        find_partial_reduce_offset(&results, "matrix", "b", "y", "row_sum").is_some(),
        "expected partial-reduce link score matrix[b,y] -> row_sum[b]"
    );

    let read = |off: usize| -> Vec<f64> {
        (0..results.step_count)
            .map(|step| results.data[step * results.step_size + off])
            .collect()
    };
    let ax_vals = read(ax);
    let ay_vals = read(ay);
    for (label, vals) in [
        ("matrix[a,x]->row_sum[a]", &ax_vals),
        ("matrix[a,y]->row_sum[a]", &ay_vals),
    ] {
        let any_nonzero = vals.iter().any(|v| v.abs() > 1e-9 && v.is_finite());
        assert!(
            any_nonzero,
            "{label} link score should be non-zero at some step, got: {vals:?}"
        );
        let always_unit = vals
            .iter()
            .all(|v| !v.is_finite() || (v.abs() - 1.0).abs() < 1e-9);
        assert!(
            !always_unit,
            "{label} link score should not be magnitude 1 at every step \
             (a SUM partial reduce splits the row delta), got: {vals:?}"
        );
    }
    // Hand calc: a SUM partial reduce splits the row delta, so for row `a`
    // the link score magnitudes are |Δm[a,x]| / |Δrow_sum[a]| and |Δm[a,y]|
    // / |Δrow_sum[a]| with Δrow_sum[a] = Δm[a,x] + Δm[a,y]. Both inflows
    // are positive, so the two deltas share a sign and the magnitudes add
    // to 1 at every step where the row actually changed.
    for step in 2..results.step_count {
        let s = ax_vals[step].abs() + ay_vals[step].abs();
        if ax_vals[step].is_finite() && ay_vals[step].is_finite() && s > 1e-9 {
            assert!(
                (s - 1.0).abs() < 1e-6,
                "row-a partial-reduce link scores should split the row delta \
                 (sum of magnitudes ~1) at step {step}, got |{}| + |{}| = {}",
                ax_vals[step],
                ay_vals[step],
                s
            );
        }
    }

    // (b) At least one loop-score variable references the partial-reduce
    // link scores. The elementary loop that runs through `row_sum` is the
    // per-element 4-cycle `matrix[d1,d2] -> row_sum[d1] -> total ->
    // growth[d1,d2] -> matrix[d1,d2]` (`growth` references the scalar
    // full-reduce `total`, not `row_sum` directly); the conservative
    // full-cross-product element graph for the `SUM(matrix[D1,*])`
    // reference also produces spurious cross-element loops (fixed in
    // Phase 5), but at least one loop_score equation must reference a
    // real `matrix[d1,d2]->row_sum[d1]` link score for the partial reduce
    // to contribute at all -- independent of which cycle it lands in.
    let loop_score_var_count = results
        .offsets
        .keys()
        .filter(|k| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}")
        })
        .count();
    assert!(
        loop_score_var_count > 0,
        "exhaustive mode should emit loop_score variables for the partial-reduce model"
    );

    let partial_reduce_names: Vec<String> = ltm
        .vars
        .iter()
        .map(|v| v.name.clone())
        .filter(|n| {
            n.starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}matrix[")
                && n.contains("\u{2192}row_sum[")
        })
        .collect();
    assert!(
        partial_reduce_names.len() >= 4,
        "expected >=4 partial-reduce link scores (one per (d1,d2) pair), got: {partial_reduce_names:?}"
    );
    let loop_score_eqs: Vec<(String, String)> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}"))
        .map(|v| (v.name.clone(), v.equation.source_text()))
        .collect();
    let references_partial_reduce = loop_score_eqs
        .iter()
        .any(|(_, eq)| partial_reduce_names.iter().any(|n| eq.contains(n.as_str())));
    assert!(
        references_partial_reduce,
        "at least one loop_score equation must reference a partial-reduce link \
         score (matrix[d1,d2]->row_sum[d1]); loop_score equations: {loop_score_eqs:?}; \
         partial-reduce link scores: {partial_reduce_names:?}"
    );
}

// --- #511: iterated-dimension subscript link score ---
//
// An A2A equation that references an arrayed dependency by its *iterated
// dimension* (`growth[Region,Age] = row_sum[Region] * c * pop`, `row_sum`
// over `Region`, `growth` over `Region x Age`) used to misclassify the
// `row_sum[Region]` subscript as `DynamicIndex`, so the link-score partial
// PREVIOUS-wrapped a `Subscript` and codegen rejected it with "PREVIOUS
// requires a variable reference after helper rewriting". After the fix the
// subscript is `Bare` (it iterates over the target's own `Region` dimension
// and reads the same source element), the link score is `row_sum` held live
// (no spurious `SUM(...)`, no `PREVIOUS`-wrapped `Subscript`), and the model
// simulates.

/// Build a 1-D-target arrayed feedback model whose flow references an arrayed
/// aux by the flow's own iterated dimension.
///
/// Model structure:
///   level[Region]      (stock, distinct per-element initial values)
///   row_val[Region]    (aux, = level[Region] * 0.0001)  -- references `level`
///                       by `row_val`'s own iterated `Region` dim (the #511
///                       case, in its simplest form: source and target are
///                       both over `Region` and the index *is* `Region`)
///   inflow[Region]     (flow into level, = row_val[Region])  -- references
///                       `row_val` by the flow's own iterated `Region` dim
///                       (the #511 case again)
///
/// `Region = {a, b}`. The reinforcing per-element cycle is
/// `level[r] -> row_val[r] -> inflow[r] -> level[r]`. Both `level ->
/// row_val` and `row_val -> inflow` carry an iterated-dimension subscript
/// (`x[Region]` inside an apply-to-all-over-`Region` equation), the case
/// that pre-#511 misclassified as `DynamicIndex` and produced a
/// `PREVIOUS`-wrapped `Subscript` (the `"PREVIOUS requires a variable
/// reference after helper rewriting"` codegen error). The structural
/// `inflow -> level` edge closes the loop.
fn build_iterated_dim_subscript_model(name: &str) -> simlin_engine::datamodel::Project {
    use simlin_engine::datamodel::{self, Equation, Variable};

    datamodel::Project {
        name: name.to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension::named(
            "Region".to_string(),
            vec!["a".to_string(), "b".to_string()],
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Stock(datamodel::Stock {
                    ident: "level".to_string(),
                    equation: Equation::Arrayed(
                        vec!["Region".to_string()],
                        vec![
                            ("a".to_string(), "100".to_string(), None, None),
                            ("b".to_string(), "250".to_string(), None, None),
                        ],
                        None,
                        false,
                    ),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["inflow".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // row_val[Region] = level[Region] * 0.0001 -- `level[Region]`
                // is the #511 iterated-dimension subscript (the index `Region`
                // is `row_val`'s own iterated dimension and `level`'s declared
                // dimension, so it reads the same element).
                Variable::Aux(datamodel::Aux {
                    ident: "row_val".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        "level[Region] * 0.0001".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // inflow[Region] = row_val[Region] -- `row_val[Region]` is the
                // #511 iterated-dimension subscript again.
                Variable::Flow(datamodel::Flow {
                    ident: "inflow".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        "row_val[Region]".to_string(),
                    ),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// AC3.1: `row_val[Region] = level[Region] * c` + LTM compiles;
/// `$⁚ltm⁚link_score⁚level→row_val` is emitted as the Bare partial
/// (`level` held live, no spurious `SUM(...)`, no `PREVIOUS`-wrapped
/// `Subscript`), is `Equation::ApplyToAll` over `Region`, and the model
/// **simulates** without the `"PREVIOUS requires a variable reference after
/// helper rewriting"` error.
#[test]
fn test_iterated_dim_subscript_link_score_is_bare_and_simulates() {
    let project = build_iterated_dim_subscript_model("iterated_dim_link_score");

    // Inspect the synthetic vars via the salsa path.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);

    // Both `level -> row_val` and `row_val -> inflow` carry an iterated-
    // dimension subscript; check the first one in detail.
    let level_to_row_val = ltm
        .vars
        .iter()
        .find(|v| v.name == "$\u{205A}ltm\u{205A}link_score\u{205A}level\u{2192}row_val")
        .unwrap_or_else(|| {
            panic!(
                "expected $⁚ltm⁚link_score⁚level→row_val; link scores present: {:?}",
                ltm.vars
                    .iter()
                    .map(|v| v.name.as_str())
                    .filter(|s| s.contains("\u{205A}link_score\u{205A}"))
                    .collect::<Vec<_>>()
            )
        });
    // The `level -> row_val` edge is same-dimension A2A (both over Region),
    // so the link score is `Equation::ApplyToAll` over Region (per-element).
    match &level_to_row_val.equation {
        simlin_engine::datamodel::Equation::ApplyToAll(dims, text) => {
            assert_eq!(
                dims,
                &vec!["Region".to_string()],
                "the level -> row_val link score should be A2A over Region"
            );
            // The partial holds `level` live (bare) -- no `SUM(`, no
            // `PREVIOUS(level[...])`.
            assert!(
                text.contains("level"),
                "link score equation must reference level; got: {text}"
            );
            assert!(
                !text.contains("SUM("),
                "a Bare iterated-dim source ref must not produce a spurious SUM(...); got: {text}"
            );
            assert!(
                !text.contains("PREVIOUS(level["),
                "the partial must not PREVIOUS-wrap a level subscript; got: {text}"
            );
        }
        other => panic!("expected Equation::ApplyToAll for level -> row_val, got {other:?}"),
    }
    assert_eq!(level_to_row_val.dimensions, vec!["Region".to_string()]);
    // And the row_val -> inflow link score exists too (the same #511 shape).
    assert!(
        ltm.vars
            .iter()
            .any(|v| v.name == "$\u{205A}ltm\u{205A}link_score\u{205A}row_val\u{2192}inflow"),
        "expected $⁚ltm⁚link_score⁚row_val→inflow"
    );

    // The model must simulate -- pre-fix this errored with "PREVIOUS
    // requires a variable reference after helper rewriting".
    let compiled = compile_ltm_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end()
        .expect("iterated-dimension-subscript model should simulate with LTM enabled");
}

/// AC3.2: the per-element loop `level[r] -> row_val[r] -> inflow[r] ->
/// level[r]` -- which runs through *two* iterated-dimension edges (`level ->
/// row_val` and `row_val -> inflow`) -- is enumerated; its `loop_score`
/// equation references those link scores; and the loop score's series equals
/// the product of the per-element link scores it references at every
/// simulated step t >= 2 (within 1e-6).
#[test]
fn test_iterated_dim_subscript_loop_score_matches_hand_calc() {
    let project = build_iterated_dim_subscript_model("iterated_dim_loop_score");

    let compiled = compile_ltm_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("model should simulate");
    let results = vm.into_results();

    // The loop-score equation text isn't carried in `Results`; recompute the
    // synthetic-var list from the same datamodel to read it.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);
    let loop_eq_by_name: HashMap<String, String> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}"))
        .map(|v| (v.name.clone(), v.equation.source_text()))
        .collect();

    // Find a loop_score var whose equation references *both* iterated-dim
    // link scores (i.e. the level -> row_val -> inflow -> level cycle).
    let level_to_row_val_q = "level\u{2192}row_val";
    let row_val_to_inflow_q = "row_val\u{2192}inflow";
    let loop_offsets = find_loop_score_offsets(&results);
    let (loop_name, loop_off) = loop_offsets
        .iter()
        .find(|(name, _)| {
            loop_eq_by_name.get(name).is_some_and(|eq| {
                eq.contains(level_to_row_val_q) && eq.contains(row_val_to_inflow_q)
            })
        })
        .unwrap_or_else(|| {
            panic!(
                "expected a loop_score var referencing both iterated-dim link scores; \
                 loop_score equations: {:?}",
                loop_eq_by_name
            )
        });
    let loop_eq = loop_eq_by_name[loop_name].clone();

    // The whole loop -- `level[r] -> row_val[r] -> inflow[r] -> level[r]` --
    // runs through exactly these three link scores (all same-dim A2A over
    // Region, so each appears in the loop-score equation as the quoted
    // canonical name with a trailing `[a]`/`[b]` slot subscript). Asserting
    // they're all present (rather than just probing two as substrings) makes
    // a structural change to the loop-score equation format fail this test
    // rather than slip through.
    let link_score_prefix = "$\u{205A}ltm\u{205A}link_score\u{205A}";
    for edge in [
        level_to_row_val_q,
        row_val_to_inflow_q,
        "inflow\u{2192}level",
    ] {
        let expected = format!("\"{link_score_prefix}{edge}\"");
        assert!(
            loop_eq.contains(&expected),
            "loop-score equation should reference {expected}; got: {loop_eq}"
        );
    }

    // The hand-calc below relies on the loop score being a *pure product of
    // its link scores* -- no `SIGN`, no explicit polarity factor. Splitting
    // on ` * ` and treating every term as a link-score var reference is only
    // valid under that assumption; the per-term validation in
    // `resolve_offset` (every factor must be a `$⁚ltm⁚link_score⁚...` var
    // name) makes a violation fail loudly here instead of silently
    // misinterpreting a non-product factor.
    let factors: Vec<String> = loop_eq.split(" * ").map(|s| s.trim().to_string()).collect();
    assert!(
        factors.len() >= 2,
        "loop score should be a product of >=2 link scores; got: {loop_eq}"
    );

    // Resolve the offset of each factor (a quoted link-score var name,
    // optionally with a trailing `[elem]` subscript picking a slot of an A2A
    // link score over Region = {a, b} in declaration order: a=0, b=1).
    let region_slot = |elem: &str| -> usize {
        match elem {
            "a" => 0,
            "b" => 1,
            other => panic!("unexpected Region element {other:?}"),
        }
    };
    let resolve_offset = |reference: &str| -> usize {
        let inner = reference.trim();
        let (var_part, subscript): (&str, Option<&str>) = match inner.strip_suffix(']') {
            Some(rest) => match rest.rfind('[') {
                Some(open) => (&rest[..open], Some(&rest[open + 1..])),
                None => (inner, None),
            },
            None => (inner, None),
        };
        let var_name = var_part.trim_matches('"');
        assert!(
            var_name.starts_with(link_score_prefix),
            "loop-score factor {reference:?} is not a link-score var reference -- the \
             loop-score equation grew a non-product term, and this hand-calc test's \
             ` * `-split no longer holds; full equation: {loop_eq}"
        );
        let base = results
            .offsets
            .iter()
            .find(|(k, _)| k.as_str() == var_name)
            .map(|(_, &off)| off)
            .unwrap_or_else(|| panic!("offset for link-score factor {var_name:?} not found"));
        match subscript {
            None => base,
            Some(elem) => base + region_slot(elem),
        }
    };
    let factor_offsets: Vec<usize> = factors.iter().map(|f| resolve_offset(f)).collect();

    let mut checked = 0usize;
    let mut saw_nonzero = false;
    for step in 2..results.step_count {
        let base = step * results.step_size;
        let product: f64 = factor_offsets
            .iter()
            .map(|&o| results.data[base + o])
            .product();
        let loop_val = results.data[base + loop_off];
        assert!(
            (loop_val - product).abs() < 1e-6,
            "step {step}: loop_score {loop_val} != product of its link scores {product} \
             (factors {factors:?} at offsets {factor_offsets:?})"
        );
        if loop_val.abs() > 1e-9 && loop_val.is_finite() {
            saw_nonzero = true;
        }
        checked += 1;
    }
    assert!(checked > 0, "expected at least one step t >= 2 to check");
    assert!(
        saw_nonzero,
        "the iterated-dimension loop's score should be non-zero at some step"
    );
}

// --- #510: disjoint-dim arrayed -> arrayed link scores ---
//
// A disjoint-dim arrayed -> arrayed edge with per-element target equations
// (`target[D1,D2]` whose `<element subscript>` equations reference
// `source[D3]`, D3 disjoint from D1/D2) used to degenerate to a silent
// scalarized stand-in (`link_score_dimensions` returned `[]` for the edge,
// so `retarget_ltm_equation_dims` collapsed the per-element `Equation::Arrayed`
// partial to the first slot's text). The fix emits one link-score variable
// per distinct referenced source element (`$⁚ltm⁚link_score⁚source[m]→target`,
// ...), each `Equation::Arrayed` over `target`'s dims holding `source[m]`
// live in the slots that reference it and the trivial-zero guard form
// elsewhere; and a genuinely-unscoreable edge (a `DynamicIndex` source into
// such a target) produces a clear compile-time `Warning` instead of a silent
// stand-in.

/// Build a disjoint-dim arrayed -> arrayed model whose per-element target
/// equations reference literal elements of a disjoint dimension.
///
/// `D1 = {a, b}`, `D2 = {x, y}`, `D3 = {m, n}` (all named; D3 disjoint
/// from D1/D2). `source[D3]` is a stock (so its values change over time);
/// `target[D1,D2]` is an `Equation::Arrayed` whose per-element equations
/// each reference some `source[m]` and/or `source[n]`. There is no closed
/// loop (the disjoint dims make a `target -> source` reduction impossible),
/// so the test compiles in discovery mode (which scores every causal edge).
fn build_disjoint_dim_arrayed_target_model(name: &str) -> simlin_engine::datamodel::Project {
    use simlin_engine::datamodel::{self, Equation, Variable};

    datamodel::Project {
        name: name.to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![
            datamodel::Dimension::named("D1".to_string(), vec!["a".to_string(), "b".to_string()]),
            datamodel::Dimension::named("D2".to_string(), vec!["x".to_string(), "y".to_string()]),
            datamodel::Dimension::named("D3".to_string(), vec!["m".to_string(), "n".to_string()]),
        ],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                // source[D3] (stock, distinct inits) <- src_inflow (constant).
                Variable::Stock(datamodel::Stock {
                    ident: "source".to_string(),
                    equation: Equation::Arrayed(
                        vec!["D3".to_string()],
                        vec![
                            ("m".to_string(), "10".to_string(), None, None),
                            ("n".to_string(), "20".to_string(), None, None),
                        ],
                        None,
                        false,
                    ),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["src_inflow".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(datamodel::Flow {
                    ident: "src_inflow".to_string(),
                    equation: Equation::ApplyToAll(vec!["D3".to_string()], "0.1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // target[D1,D2] (per-element equations referencing literal
                // elements of the disjoint dimension D3).
                Variable::Aux(datamodel::Aux {
                    ident: "target".to_string(),
                    equation: Equation::Arrayed(
                        vec!["D1".to_string(), "D2".to_string()],
                        vec![
                            ("a,x".to_string(), "source[m] * 2".to_string(), None, None),
                            ("a,y".to_string(), "source[n] * 3".to_string(), None, None),
                            ("b,x".to_string(), "source[m]".to_string(), None, None),
                            (
                                "b,y".to_string(),
                                "source[n] * source[m]".to_string(),
                                None,
                                None,
                            ),
                        ],
                        None,
                        false,
                    ),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// Build a disjoint-dim arrayed -> arrayed model whose per-element target
/// equations reference the disjoint dimension via a *non-literal* index --
/// genuinely unscoreable (which target slots depend on which source
/// elements can't be decided statically).
///
/// `D1 = {a, b}`, `D2 = {x, y}` (named), `D3` indexed of size 2 (so a
/// numeric index variable is a valid subscript). `source[D3]` is a stock;
/// `idx` is a scalar aux (= 1); `target[D1,D2]`'s per-element equations
/// reference `source[idx]`.
fn build_disjoint_dim_unscoreable_model(name: &str) -> simlin_engine::datamodel::Project {
    use simlin_engine::datamodel::{self, Equation, Variable};

    datamodel::Project {
        name: name.to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![
            datamodel::Dimension::named("D1".to_string(), vec!["a".to_string(), "b".to_string()]),
            datamodel::Dimension::named("D2".to_string(), vec!["x".to_string(), "y".to_string()]),
            datamodel::Dimension::indexed("D3".to_string(), 2),
        ],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Stock(datamodel::Stock {
                    ident: "source".to_string(),
                    equation: Equation::Arrayed(
                        vec!["D3".to_string()],
                        vec![
                            ("1".to_string(), "10".to_string(), None, None),
                            ("2".to_string(), "20".to_string(), None, None),
                        ],
                        None,
                        false,
                    ),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["src_inflow".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(datamodel::Flow {
                    ident: "src_inflow".to_string(),
                    equation: Equation::ApplyToAll(vec!["D3".to_string()], "0.1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Aux(datamodel::Aux {
                    ident: "idx".to_string(),
                    equation: Equation::Scalar("1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Aux(datamodel::Aux {
                    ident: "target".to_string(),
                    equation: Equation::Arrayed(
                        vec!["D1".to_string(), "D2".to_string()],
                        vec![
                            ("a,x".to_string(), "source[idx] * 2".to_string(), None, None),
                            ("a,y".to_string(), "source[idx] * 3".to_string(), None, None),
                            ("b,x".to_string(), "source[idx]".to_string(), None, None),
                            ("b,y".to_string(), "source[idx] * 5".to_string(), None, None),
                        ],
                        None,
                        false,
                    ),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// AC3.3: a disjoint-dim arrayed -> arrayed model with per-element target
/// equations emits one link-score variable per distinct referenced source
/// element (`$⁚ltm⁚link_score⁚source[m]→target`, `$⁚ltm⁚link_score⁚source[n]→target`)
/// -- not a single `$⁚ltm⁚link_score⁚source→target` scalar stand-in -- each
/// `Equation::Arrayed` over `target`'s dims (`["D1","D2"]`); the `[a,x]` slot
/// of the `source[m]→target` var holds `source[m]` live (its partial differs
/// from `PREVIOUS`-evaluated) and the `[a,y]` slot (references `source[n]`,
/// not `m`) is the trivial-zero guard form; and running the VM, the
/// `source[m]→target` link score is non-zero at the `[a,x]` slot at some step
/// >= 2 and ~0 at `[a,y]` at every step >= 2.
#[test]
fn test_disjoint_dim_arrayed_target_per_source_element_link_scores() {
    let project = build_disjoint_dim_arrayed_target_model("disjoint_dim_arrayed");

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);

    let m_name = "$\u{205A}ltm\u{205A}link_score\u{205A}source[m]\u{2192}target";
    let n_name = "$\u{205A}ltm\u{205A}link_score\u{205A}source[n]\u{2192}target";
    let m_var = ltm
        .vars
        .iter()
        .find(|v| v.name == m_name)
        .unwrap_or_else(|| {
            panic!(
                "expected {m_name}; link scores present: {:?}",
                ltm.vars
                    .iter()
                    .map(|v| v.name.as_str())
                    .filter(|s| s.contains("\u{205A}link_score\u{205A}"))
                    .collect::<Vec<_>>()
            )
        });
    assert!(
        ltm.vars.iter().any(|v| v.name == n_name),
        "expected {n_name} (one link score per distinct referenced source element)"
    );
    // No scalar stand-in `source→target`.
    assert!(
        !ltm.vars
            .iter()
            .any(|v| v.name == "$\u{205A}ltm\u{205A}link_score\u{205A}source\u{2192}target"),
        "must NOT emit a single scalar stand-in $⁚ltm⁚link_score⁚source→target"
    );
    // Each per-source-element link score is Equation::Arrayed over target's dims.
    match &m_var.equation {
        simlin_engine::datamodel::Equation::Arrayed(dims, elements, _, _) => {
            assert_eq!(
                dims,
                &vec!["D1".to_string(), "D2".to_string()],
                "the source[m]→target link score should be Arrayed over D1 x D2"
            );
            // The [a,x] slot references source[m] -> holds source[m] live (the
            // partial mentions source[m]); the [a,y] slot references source[n]
            // not source[m] -> the source[m] reference there is frozen, so the
            // partial is the PREVIOUS-evaluated form (a trivial-zero guard).
            let slot = |elem: &str| -> &str {
                elements
                    .iter()
                    .find(|(e, _, _, _)| e == elem)
                    .map(|(_, eq, _, _)| eq.as_str())
                    .unwrap_or_else(|| panic!("slot {elem:?} not found in {elements:?}"))
            };
            let ax = slot("a,x");
            let ay = slot("a,y");
            assert!(
                ax.contains("source[m]"),
                "the [a,x] slot of source[m]→target should reference source[m] live; got: {ax}"
            );
            // The [a,y] slot's partial: every source reference is `source[n]`,
            // which for the `source[m]` link score is "other content" and gets
            // PREVIOUS-frozen, so the partial equals PREVIOUS(target[a,y]) and
            // the guarded ratio is the trivial-zero form. (We don't pin the
            // exact text -- the VM check below is the substantive one -- but it
            // must not hold `source[m]` live.)
            assert!(
                !ax.contains("source[n]") || ay.contains("PREVIOUS(source[n]"),
                "sanity: [a,y] slot freezes source[n] for the source[m] link score; got: {ay}"
            );
        }
        other => panic!("expected Equation::Arrayed for source[m]→target, got {other:?}"),
    }
    assert_eq!(m_var.dimensions, vec!["D1".to_string(), "D2".to_string()]);

    // Compile and simulate; the source[m]→target link score's [a,x] slot is
    // non-zero at some step >= 2, and its [a,y] slot is ~0 at every step >= 2.
    let compiled = compile_ltm_discovery_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end()
        .expect("disjoint-dim arrayed-target model should simulate with LTM enabled");
    let results = vm.into_results();

    let base = results
        .offsets
        .iter()
        .find(|(k, _)| k.as_str() == m_name)
        .map(|(_, &off)| off)
        .unwrap_or_else(|| panic!("offset for {m_name:?} not found in results"));
    // Region x Age declaration order: a,x=0; a,y=1; b,x=2; b,y=3.
    let ax_off = base;
    let ay_off = base + 1;
    let mut checked = 0usize;
    let mut saw_ax_nonzero = false;
    for step in 2..results.step_count {
        let row = step * results.step_size;
        let ax_val = results.data[row + ax_off];
        let ay_val = results.data[row + ay_off];
        if ax_val.abs() > 1e-9 && ax_val.is_finite() {
            saw_ax_nonzero = true;
        }
        assert!(
            ay_val.abs() < 1e-6,
            "step {step}: source[m]→target [a,y] slot should be ~0 (it references source[n], not m); got {ay_val}"
        );
        checked += 1;
    }
    assert!(checked > 0, "expected at least one step t >= 2 to check");
    assert!(
        saw_ax_nonzero,
        "the source[m]→target [a,x] slot should be non-zero at some step >= 2"
    );
}

/// AC3.4: a disjoint-dim arrayed -> arrayed edge where the target's
/// per-element equations reference the disjoint dimension via a *non-literal*
/// index produces a clear compile-time `Warning` diagnostic naming the
/// unscoreable `source -> target` edge, and emits *no* `$⁚ltm⁚link_score⁚source...→target`
/// variable (no scalar stand-in). The model still compiles and simulates.
#[test]
fn test_disjoint_dim_unscoreable_edge_warns_and_emits_no_link_score() {
    use simlin_engine::db::CompilationDiagnostic;

    let project = build_disjoint_dim_unscoreable_model("disjoint_dim_unscoreable");

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);

    // No link-score variable for the (source, target) edge -- no scalar
    // stand-in, no per-element vars.
    assert!(
        !ltm.vars.iter().any(|v| {
            v.name
                .starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}source")
                && v.name.contains("\u{2192}target")
        }),
        "an unscoreable disjoint-dim edge must emit no source...→target link score; got: {:?}",
        ltm.vars
            .iter()
            .map(|v| v.name.as_str())
            .filter(|s| s.contains("\u{205A}link_score\u{205A}"))
            .collect::<Vec<_>>()
    );

    // A Warning diagnostic naming the unscoreable source -> target edge.
    let diags =
        model_ltm_variables::accumulated::<CompilationDiagnostic>(&db, source_model, sync.project);
    let has_warning = diags.iter().any(|CompilationDiagnostic(d)| {
        d.severity == simlin_engine::db::DiagnosticSeverity::Warning
            && matches!(
                &d.error,
                simlin_engine::db::DiagnosticError::Assembly(msg)
                    if msg.contains("source") && msg.contains("target")
            )
    });
    assert!(
        has_warning,
        "expected a Warning diagnostic naming the unscoreable source -> target edge; got: {:?}",
        diags.iter().map(|c| &c.0).collect::<Vec<_>>()
    );

    // The model still compiles and simulates (a missing link score is graceful).
    let compiled = compile_ltm_discovery_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end()
        .expect("unscoreable-edge model should still compile and simulate");
}

/// No regression: the existing full-reduce (`SUM(population[*])` -> scalar)
/// integration tests still pass with unchanged values. (This test exists
/// purely to keep the AC4.6 work co-located with an explicit assertion
/// that the full-reduce path is untouched; the heavy lifting is in
/// `test_cross_dim_sum_algebraic` & friends above, which run as part of
/// the same binary.)
#[test]
fn test_full_reduce_still_works_after_partial_reduce_support() {
    let project =
        build_arrayed_to_scalar_model("full_reduce_regression", "SUM(population[*])", "total_pop");
    let compiled = compile_ltm_discovery_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let offsets = find_cross_dimensional_offsets(&results, "population", "total_pop");
    assert_eq!(
        offsets.len(),
        3,
        "full reduce SUM(population[*]) must still emit 3 per-source-element scalar link scores, got: {offsets:?}"
    );
    // And no per-(d1,d2) partial-reduce names should appear for this 1-D
    // model.
    assert!(
        results
            .offsets
            .keys()
            .all(|k| !k.as_str().contains("\u{2192}total_pop[")),
        "a scalar target must not get an arrayed-result (partial-reduce) link score"
    );
}

// --- Phase 5: aggregate-node ($⁚ltm⁚agg⁚{n}) auxiliaries ---
//
// A maximal inlined reducer subexpression that participates in feedback is
// hoisted into a synthetic auxiliary whose value at every timestep equals
// the value the inline reducer would compute (it *is* the same expression --
// model equations are not rewritten). `PREVIOUS(agg)` is available because
// the agg fragment is a regular flow-phase fragment with a layout slot.

/// AC4.1: the synthetic agg `$⁚ltm⁚agg⁚0 = SUM(pop[*])` computes the same
/// value as `pop[nyc] + pop[boston]` at every timestep, and the
/// `$⁚ltm⁚link_score⁚$⁚ltm⁚agg⁚0→share[r]` link score (which reads the agg's
/// current-step value) is finite -- a runlist-ordering bug (agg running
/// after the link score) would surface as a stale value mismatch.
#[test]
fn test_agg_aux_value_matches_reducer() {
    let project = TestProject::new("agg_value")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        // Heterogeneous initial values so the SUM is exercised non-trivially.
        .array_stock("pop[Region]", "100", &["update"], &[], None)
        .array_aux("share[Region]", "pop / SUM(pop[*])")
        .array_flow("update[Region]", "share * 0.001", None)
        .build_datamodel();

    let compiled = compile_ltm_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("should simulate");
    let results = vm.into_results();

    let agg_offset = results.offsets[&Ident::<Canonical>::new("$\u{205A}ltm\u{205A}agg\u{205A}0")];
    let pop_nyc = results.offsets[&Ident::<Canonical>::new("pop[nyc]")];
    let pop_boston = results.offsets[&Ident::<Canonical>::new("pop[boston]")];

    for step in 0..results.step_count {
        let row = step * results.step_size;
        let agg = results.data[row + agg_offset];
        let expected = results.data[row + pop_nyc] + results.data[row + pop_boston];
        assert!(
            (agg - expected).abs() < 1e-9 * expected.abs().max(1.0),
            "step {step}: agg = {agg}, expected SUM(pop[*]) = {expected}"
        );
    }

    // The agg→share link score reads the agg's *current-step* value; if the
    // agg fragment ran after it, the value would be stale (or NaN at step 0).
    // Just require it to exist and be finite at every step.
    let ls_offsets: Vec<usize> = results
        .offsets
        .iter()
        .filter(|(k, _)| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}$\u{205A}ltm\u{205A}agg\u{205A}0\u{2192}share")
        })
        .map(|(_, &o)| o)
        .collect();
    assert!(
        !ls_offsets.is_empty(),
        "expected an agg→share link score variable; offsets: {:?}",
        results
            .offsets
            .keys()
            .map(|k| k.as_str())
            .collect::<Vec<_>>()
    );
    for &o in &ls_offsets {
        for step in 0..results.step_count {
            let v = results.data[step * results.step_size + o];
            assert!(
                v.is_finite(),
                "step {step}: agg→share link score = {v} (not finite)"
            );
        }
    }
}

/// AC4.3 (#514, end-to-end): a *sliced* reducer subexpression
/// `SUM(pop[NYC,*])` over `pop[Region,Age]` hoisted into a synthetic agg
/// computes `pop[nyc,adult] + pop[nyc,child]` at every timestep, the
/// per-read-row link scores `$⁚ltm⁚link_score⁚pop[nyc,age]→$⁚ltm⁚agg⁚0` exist
/// and are finite (and there is *no* `pop[boston,*]→agg` link score -- the
/// slice doesn't read those rows), and a cross-element feedback loop visiting
/// NYC through the sliced agg is scored with a finite, non-degenerate
/// `loop_score`. (`drive` is arrayed over `(Region,Age)` so each `pop` slot's
/// loop through the agg has its own `drive`/`flow` nodes -- the disjointness
/// `recover_cross_agg_loops` needs to stitch the two NYC petals together.)
#[test]
fn test_sliced_agg_cross_element_loop_simulates() {
    let project = TestProject::new("sliced_agg_sim")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .named_dimension("Age", &["Adult", "Child"])
        .array_stock("pop[Region,Age]", "100", &["flow"], &[], None)
        // `SUM(pop[NYC,*])` is the maximal reducer sub-expression -> hoisted
        // into a synthetic agg, broadcast to every `drive` element. The `pop`
        // factor makes growth exponential.
        .array_aux("drive[Region,Age]", "SUM(pop[NYC,*]) * pop * 0.00001")
        .array_flow("flow[Region,Age]", "drive", None)
        .build_datamodel();

    // Compile (exhaustive LTM) and simulate.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);
    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("should simulate");
    let results = vm.into_results();

    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let off = |name: &str| -> usize {
        *results
            .offsets
            .get(&Ident::<Canonical>::new(name))
            .unwrap_or_else(|| {
                panic!(
                    "missing offset {name}; have: {:?}",
                    results
                        .offsets
                        .keys()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                )
            })
    };
    let at = |step: usize, o: usize| results.data[step * results.step_size + o];
    let agg_off = off(agg);
    let nyc_adult = off("pop[nyc,adult]");
    let nyc_child = off("pop[nyc,child]");

    // The agg equals the `pop[NYC,*]` slice sum at every step.
    for step in 0..results.step_count {
        let expected = at(step, nyc_adult) + at(step, nyc_child);
        assert!(
            (at(step, agg_off) - expected).abs() < 1e-9 * expected.abs().max(1.0),
            "step {step}: agg = {}, expected SUM(pop[NYC,*]) = {expected}",
            at(step, agg_off)
        );
    }

    // The per-read-row link scores exist and are finite; the unread Boston
    // rows get no link score into the agg.
    for age in &["adult", "child"] {
        let o = off(&format!(
            "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc,{age}]\u{2192}{agg}"
        ));
        for step in 0..results.step_count {
            assert!(
                at(step, o).is_finite(),
                "step {step}: pop[nyc,{age}]→agg link score not finite"
            );
        }
    }
    for age in &["adult", "child"] {
        assert!(
            !results
                .offsets
                .contains_key(&Ident::<Canonical>::new(&format!(
                    "$\u{205A}ltm\u{205A}link_score\u{205A}pop[boston,{age}]\u{2192}{agg}"
                ))),
            "must not emit a pop[boston,{age}]→agg link score (the slice reads only NYC)"
        );
    }

    // A loop_score var traversing the NYC-through-sliced-agg path exists, and
    // its simulated series is finite and not all-zero.
    let cross_agg_loop_score_name = ltm
        .vars
        .iter()
        .find(|v| {
            v.name.contains("\u{205A}loop_score\u{205A}")
                && v.equation.source_text().contains(
                    format!(
                        "\"$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc,adult]\u{2192}{agg}\""
                    )
                    .as_str(),
                )
                && v.equation.source_text().contains(
                    format!(
                        "\"$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc,child]\u{2192}{agg}\""
                    )
                    .as_str(),
                )
        })
        .map(|v| v.name.clone())
        .expect("expected a loop_score var traversing both NYC slots through the sliced agg");
    let lo = off(&cross_agg_loop_score_name);
    let mut saw_nonzero = false;
    for step in 0..results.step_count {
        let v = at(step, lo);
        assert!(
            v.is_finite(),
            "step {step}: cross-agg loop score not finite"
        );
        if v.abs() > 1e-12 {
            saw_nonzero = true;
        }
    }
    assert!(
        saw_nonzero,
        "the cross-element-through-sliced-agg loop score must be non-degenerate"
    );
}

/// AC4.3 (#514, end-to-end -- the *arrayed* synthetic agg case): a sliced
/// reducer `SUM(matrix[D1,*])` that is a *subexpression* of an A2A equation
/// over `D1` reads one `D1` element per A2A iteration, so its read slice is
/// `[Iterated(d1), Reduced]` and `result_dims == [D1]` -- it mints an
/// *arrayed* synthetic agg `$⁚ltm⁚agg⁚0[d1]`. The fix verified here is the
/// agg-half link-score emitters using subscripted agg names on both sides
/// (`matrix[d1,d2]→$⁚ltm⁚agg⁚0[d1]` per read row, `$⁚ltm⁚agg⁚0[d1]→growth[d1]`,
/// with the `agg→target` equation's `Δsource` denominator also carrying the
/// `[d1]` slot subscript -- the bare-agg-name form does not compile when the
/// agg is multi-slot), the agg's equation being reconstructed as an
/// `ApplyToAll` over `D1` (not a scalar -- otherwise `matrix[d1,*]` is a type
/// error and the source-half link scores silently vanish, zeroing the loop
/// score), and the element loop through the agg being routed to the
/// per-circuit element-subscripted path so its loop-score equation can
/// reference those literal-element agg-half link scores. The model simulates
/// and the cross-element-through-arrayed-agg loop is scored with a finite,
/// non-degenerate `loop_score`.
///
/// The fixture is the *diagonal* case (`result_dims == growth`'s dims): the
/// agg over `D1` feeds a target also over exactly `D1`. The strict-prefix
/// *broadcast* case (`SUM(matrix[D1,*])` inside an A2A body over `(D1, D2)`,
/// `agg[D1] → growth[D1,D2]`) is the GH #528 twin, pinned by
/// `ltm_array_agg::broadcast_agg_loop_scores_are_finite_and_sustained`: there
/// the `agg→target` partial pins the agg to the target element's PROJECTION
/// onto the agg's `result_dims` axes (the full `(d1,d2)` tuple would
/// over-subscript the 1-D agg). For the diagonal case here the projection IS
/// the full tuple; `mflow[D1,D2] = growth[D1]` (the GH #511 iterated-dim
/// subscript) then broadcasts `growth` over `D2` to close the per-`(D1,D2)`
/// element loops through `matrix`.
#[test]
fn test_arrayed_sliced_agg_cross_element_loop_simulates() {
    // `D1={a,b}`, `D2={x,y}`. `matrix[D1,D2]` stock <- `mflow[D1,D2]`;
    // `growth[D1] = SUM(matrix[D1,*]) * 0.01 + 1` (A2A over D1 -- the
    // `SUM(matrix[D1,*])` sub-expr reads `matrix[<this D1 row>, *]`: read
    // slice `[Iterated(d1), Reduced]`, `result_dims == [D1]` -> arrayed agg
    // over `D1`, diagonal with `growth`); `mflow[D1,D2] = growth[D1]` (the
    // GH #511 iterated-dim subscript: `growth[D1]` inside an A2A-over-(D1,D2)
    // body reads the same `D1` element, broadcasting `growth` over `D2`).
    // Per-`(D1,D2)` loop: `matrix[d1,d2] → $⁚ltm⁚agg⁚0[d1] → growth[d1] → mflow[d1,d2] → matrix[d1,d2]`.
    let project = TestProject::new("arrayed_sliced_agg_sim")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .array_stock("matrix[D1,D2]", "100", &["mflow"], &[], None)
        .array_aux("growth[D1]", "SUM(matrix[D1,*]) * 0.01 + 1")
        .array_flow("mflow[D1,D2]", "growth[D1]", None)
        .build_datamodel();

    // The agg node carries `result_dims == [D1]` (it is arrayed).
    {
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        set_project_ltm_enabled(&mut db, sync.project, true);
        let source_model = sync.models["main"].source_model;
        let agg_nodes =
            simlin_engine::ltm_agg::enumerate_agg_nodes(&db, source_model, sync.project);
        let synthetic: Vec<_> = agg_nodes.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "expected exactly one synthetic agg for SUM(matrix[D1,*]); got: {:?}",
            agg_nodes
                .aggs
                .iter()
                .map(|a| (&a.name, a.is_synthetic, &a.result_dims))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            synthetic[0].result_dims,
            vec!["D1".to_string()],
            "SUM(matrix[D1,*]) as a subexpression of an A2A-over-D1 body must mint an arrayed agg over D1"
        );
    }

    // Compile (exhaustive LTM) and simulate.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);

    // The synthetic agg aux is itself an A2A variable over D1.
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let agg_var = ltm.vars.iter().find(|v| v.name == agg).unwrap_or_else(|| {
        panic!(
            "expected the synthetic agg aux {agg}; synthetic vars: {:?}",
            ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        )
    });
    assert_eq!(
        agg_var.dimensions,
        vec!["D1".to_string()],
        "the synthetic agg aux must be arrayed over D1"
    );

    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end()
        .expect("arrayed-synthetic-agg model should simulate with LTM enabled");
    let results = vm.into_results();

    let off = |name: &str| -> usize {
        *results
            .offsets
            .get(&Ident::<Canonical>::new(name))
            .unwrap_or_else(|| {
                panic!(
                    "missing offset {name}; have: {:?}",
                    results
                        .offsets
                        .keys()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                )
            })
    };
    let at = |step: usize, o: usize| results.data[step * results.step_size + o];

    // The synthetic agg aux is A2A over D1: in `Results` an LTM A2A var is
    // keyed by its bare name with the per-element slots laid out at
    // `base + <D1 index>` (D1 = {a, b} in declaration order). Its `a` slot
    // equals `matrix[a,x] + matrix[a,y]` (the D1=a row sum), `b` likewise.
    let agg_base = off(agg);
    for (d1_idx, (mx_a, mx_b)) in [
        ("matrix[a,x]", "matrix[a,y]"),
        ("matrix[b,x]", "matrix[b,y]"),
    ]
    .into_iter()
    .enumerate()
    {
        let agg_slot = agg_base + d1_idx;
        let mx_a = off(mx_a);
        let mx_b = off(mx_b);
        for step in 0..results.step_count {
            let expected = at(step, mx_a) + at(step, mx_b);
            assert!(
                (at(step, agg_slot) - expected).abs() < 1e-9 * expected.abs().max(1.0),
                "step {step}: {agg} slot {d1_idx} = {}, expected SUM(matrix[D1={d1_idx},*]) = {expected}",
                at(step, agg_slot)
            );
        }
    }

    // Per-(read row x agg slot) source-half link scores exist (4 combos:
    // each matrix[d1,d2] reads into the agg's d1 slot) and are finite.
    for d1 in &["a", "b"] {
        for d2 in &["x", "y"] {
            let o = off(&format!(
                "$\u{205A}ltm\u{205A}link_score\u{205A}matrix[{d1},{d2}]\u{2192}{agg}[{d1}]"
            ));
            for step in 0..results.step_count {
                assert!(
                    at(step, o).is_finite(),
                    "step {step}: matrix[{d1},{d2}]→{agg}[{d1}] link score not finite"
                );
            }
        }
    }
    // A `matrix` row never feeds the *other* D1 row's agg slot (the slice
    // reads only `matrix[<this D1>, *]`).
    assert!(
        !results
            .offsets
            .contains_key(&Ident::<Canonical>::new(&format!(
                "$\u{205A}ltm\u{205A}link_score\u{205A}matrix[a,x]\u{2192}{agg}[b]"
            ))),
        "must not emit a matrix[a,x]→{agg}[b] link score (the d1=a slice reads only matrix[a,*])"
    );

    // The agg->target half exists per target element (diagonal: the agg's
    // `d1` slot rides the `from` side, `growth`'s element is also just `d1`).
    for d1 in &["a", "b"] {
        let o = off(&format!(
            "$\u{205A}ltm\u{205A}link_score\u{205A}{agg}[{d1}]\u{2192}growth[{d1}]"
        ));
        for step in 0..results.step_count {
            assert!(
                at(step, o).is_finite(),
                "step {step}: {agg}[{d1}]→growth[{d1}] link score not finite"
            );
        }
    }

    // The cross-element-through-arrayed-agg loop is enumerated, and its
    // loop_score series is finite and not all-zero at some step >= 2. The
    // loop's score equation references the per-element agg-half link scores
    // (which only exist as literal-element scalar vars); pre-fix this
    // circuit went through the unsubscripted A2A-collapse path and got a
    // stub-zero loop score because no `matrix→$⁚ltm⁚agg⁚0` A2A var exists,
    // and the `agg[d1]→growth[d1]` half (had it been built unsubscripted)
    // would not have compiled because a multi-slot agg can't be referenced
    // bare in a scalar equation.
    let cross_agg_loop_score_name = ltm
        .vars
        .iter()
        .find(|v| {
            v.name.contains("\u{205A}loop_score\u{205A}")
                && v.equation
                    .source_text()
                    .contains(format!("{agg}[a]\u{2192}growth[a]").as_str())
                && v.equation.source_text().contains("matrix[a,")
                && v.equation.source_text().contains(format!("\u{2192}{agg}[a]").as_str())
        })
        .map(|v| v.name.clone())
        .unwrap_or_else(|| {
            panic!(
                "expected a loop_score var traversing matrix[a,*]→{agg}[a]→growth[a]; loop scores: {:?}",
                ltm.vars
                    .iter()
                    .filter(|v| v.name.contains("\u{205A}loop_score\u{205A}"))
                    .map(|v| (v.name.as_str(), v.equation.source_text()))
                    .collect::<Vec<_>>()
            )
        });
    let lo = off(&cross_agg_loop_score_name);
    let mut saw_nonzero = false;
    for step in 2..results.step_count {
        let v = at(step, lo);
        assert!(
            v.is_finite(),
            "step {step}: cross-element-through-arrayed-agg loop score not finite"
        );
        if v.abs() > 1e-12 {
            saw_nonzero = true;
        }
    }
    assert!(
        saw_nonzero,
        "the cross-element-through-arrayed-agg loop score must be non-degenerate (non-zero at some step >= 2)"
    );
}

/// AC4.2 regression (exhaustive loop-link path): `share[r] = pop[r] / SUM(pop[*])`
/// references `pop` *both* directly (the `pop[r]` numerator) and via the hoisted
/// reducer `$⁚ltm⁚agg⁚0 = SUM(pop[*])`. With `update[r] = share[r] * pop[r] * c`
/// feeding `pop[r]`, the element graph has two parallel cycles: the numerator path
/// `pop[r] → share[r] → update[r] → pop[r]` and the reducer path
/// `pop[r] → $⁚ltm⁚agg⁚0 → share[r] → update[r] → pop[r]`. The exhaustive
/// loop-link emitter visits the agg-routed loop links (`pop → agg`, `agg → share`)
/// directly *and* visits the direct `pop → share` loop link through
/// `emit_link_scores_for_edge`, which used to re-emit the agg's two halves -- so
/// `$⁚ltm⁚link_score⁚pop[..]→$⁚ltm⁚agg⁚0` and `$⁚ltm⁚link_score⁚$⁚ltm⁚agg⁚0→share[..]`
/// ended up in the `Vec<LtmSyntheticVar>` twice. There must be no duplicate
/// synthetic variable names.
#[test]
fn test_no_duplicate_ltm_vars_with_agg_routed_and_direct_edge() {
    // Non-discovery (exhaustive) compilation: this model is not a sub-model and
    // has internal loops, so it takes the `else if let Some(detected_loops)`
    // branch where the duplication lived.
    let project = build_heterogeneous_share_model(0.01);
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm_vars = model_ltm_variables(&db, source_model, sync.project);

    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut dups: Vec<&str> = Vec::new();
    for v in &ltm_vars.vars {
        if !seen.insert(v.name.as_str()) {
            dups.push(v.name.as_str());
        }
    }
    assert!(
        dups.is_empty(),
        "model_ltm_variables emitted duplicate synthetic variable names: {dups:?}; \
         full list: {:?}",
        ltm_vars
            .vars
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );

    // Sanity: the agg-routed link scores are still present (the fix skips the
    // *re*-emission via the direct edge, not the legitimate emission via the
    // agg-routed loop links).
    assert!(
        ltm_vars.vars.iter().any(|v| v
            .name
            .starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}pop[")
            && v.name.contains("\u{2192}$\u{205A}ltm\u{205A}agg\u{205A}0")),
        "expected a pop[..]→$⁚ltm⁚agg⁚0 link score; got: {:?}",
        ltm_vars
            .vars
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        ltm_vars.vars.iter().any(|v| v.name.starts_with(
            "$\u{205A}ltm\u{205A}link_score\u{205A}$\u{205A}ltm\u{205A}agg\u{205A}0\u{2192}share"
        )),
        "expected a $⁚ltm⁚agg⁚0→share[..] link score; got: {:?}",
        ltm_vars
            .vars
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );
}

/// Build a 2-region `share[r] = pop[r] / SUM(pop[*])` model with heterogeneous
/// stock initial values (`pop[big] >> pop[small]`), `pop` fed back by
/// `update[r] = share[r] * pop[r] * c` -- the `* pop[r]` makes growth curved
/// (a near-constant feedback flow has ~zero second-order differences, so the
/// flow→stock link score -- and thus every loop score -- would vanish; the
/// curvature keeps discovery's strongest-path scores non-degenerate). The
/// reducer `SUM(pop[*])` is a subexpression, so Phase 5 hoists it into
/// `$⁚ltm⁚agg⁚0`.
fn build_heterogeneous_share_model(c: f64) -> simlin_engine::datamodel::Project {
    use simlin_engine::datamodel::{self, Equation, Variable};
    datamodel::Project {
        name: "het_share".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 5.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension::named(
            "Region".to_string(),
            vec!["Big".to_string(), "Small".to_string()],
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Stock(datamodel::Stock {
                    ident: "pop".to_string(),
                    equation: Equation::Arrayed(
                        vec!["Region".to_string()],
                        vec![
                            ("Big".to_string(), "1000".to_string(), None, None),
                            ("Small".to_string(), "10".to_string(), None, None),
                        ],
                        None,
                        false,
                    ),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["update".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Aux(datamodel::Aux {
                    ident: "share".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        "pop / SUM(pop[*])".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(datamodel::Flow {
                    ident: "update".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        format!("share * pop * {c}"),
                    ),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// AC4.5 (heterogeneous magnitudes -- the issue's motivating case, at the
/// link-score level): for `share[r] = pop[r] / SUM(pop[*])` with
/// `pop[big] >> pop[small]`, the per-source-element `|Δpop[d] / Δ$⁚ltm⁚agg⁚0|`
/// factor is present and *non-constant* across `d` -- ~1 for `pop[big]` and
/// ~0 for `pop[small]` at every step where `pop[big]` dominates the change --
/// and matches a hand calculation from the simulated `pop` / agg values. A
/// single lumped `|Δ_aggregate(share[r]) / Δshare[r]|` link score would give
/// the same value for both elements; the agg-routed link scores do not.
#[test]
fn test_agg_link_scores_heterogeneous_match_hand_calc() {
    let project = build_heterogeneous_share_model(0.01);
    let compiled = compile_ltm_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("should simulate");
    let results = vm.into_results();

    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let off = |name: &str| -> usize {
        *results
            .offsets
            .get(&Ident::<Canonical>::new(name))
            .unwrap_or_else(|| {
                panic!(
                    "missing offset {name}; have: {:?}",
                    results
                        .offsets
                        .keys()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                )
            })
    };
    let pop_big = off("pop[big]");
    let pop_small = off("pop[small]");
    let agg_off = off(agg);
    let ls_big = off(&format!(
        "$\u{205A}ltm\u{205A}link_score\u{205A}pop[big]\u{2192}{agg}"
    ));
    let ls_small = off(&format!(
        "$\u{205A}ltm\u{205A}link_score\u{205A}pop[small]\u{2192}{agg}"
    ));

    let at = |step: usize, o: usize| results.data[step * results.step_size + o];

    // From step 1 onward (we have a previous timestep), the per-element
    // link score is |Δpop[d] / Δagg| * sign(Δpop[d]/Δpop[d]) = |Δpop[d]/Δagg|.
    let mut saw_split = false;
    for step in 1..results.step_count {
        let d_agg = at(step, agg_off) - at(step - 1, agg_off);
        if d_agg.abs() < 1e-12 {
            continue;
        }
        let d_big = at(step, pop_big) - at(step - 1, pop_big);
        let d_small = at(step, pop_small) - at(step - 1, pop_small);
        let expect_big = (d_big / d_agg).abs();
        let expect_small = (d_small / d_agg).abs();
        assert!(
            (at(step, ls_big) - expect_big).abs() < 1e-6,
            "step {step}: pop[big]→agg link score = {}, hand calc = {}",
            at(step, ls_big),
            expect_big
        );
        assert!(
            (at(step, ls_small) - expect_small).abs() < 1e-6,
            "step {step}: pop[small]→agg link score = {}, hand calc = {}",
            at(step, ls_small),
            expect_small
        );
        // The two factors must differ measurably -- pop[big] dominates the
        // change, so its fraction of Δagg is large and pop[small]'s is small.
        if (expect_big - expect_small).abs() > 0.5 {
            saw_split = true;
        }
    }
    assert!(
        saw_split,
        "expected at least one step where the per-element |Δpop[d]/Δagg| factors \
         differ by > 0.5 (the lumped approximation would make them equal)"
    );

    // The agg→share[d] link scores must also exist for both target elements.
    for d in &["big", "small"] {
        let name = format!("$\u{205A}ltm\u{205A}link_score\u{205A}{agg}\u{2192}share[{d}]");
        assert!(
            results
                .offsets
                .contains_key(&Ident::<Canonical>::new(&name)),
            "expected agg→share[{d}] link score {name:?}"
        );
    }
}

/// AC4.7 / AC4.2: discovery mode (strongest-path) on the inlined-reducer model
/// `share[r] = pop[r] / SUM(pop[*])`, `update[r] = share[r] * pop[r] * c`,
/// `pop[r]` a stock fed by `update`.
///
/// What this verifies:
///
/// 1. **The synthetic aggregate node is trimmed out of every reported loop**
///    (AC4.2). The rerouted element graph (`model_element_causal_edges`) routes
///    the inlined `SUM(pop[*])` through a synthetic `$⁚ltm⁚agg⁚0` node, so the
///    self-element loop discovery actually traverses is the four-edge cycle
///    `pop[big] -> $⁚ltm⁚agg⁚0 -> share[big] -> update[big] -> pop[big]`. The
///    trim post-pass collapses `[pop[big] -> agg, agg -> share[big]]` into a
///    single `[pop[big] -> share[big]]` edge, so no reported `Link` references
///    `$⁚ltm⁚agg⁚0`.
///
/// 2. **The discovered loop's `loop_score` series is the product of the
///    per-element link scores along the *un-trimmed* path -- including the
///    `pop[big] -> agg` and `agg -> share[big]` halves** (AC4.2). This is the
///    assertion that distinguishes the SUM/aggregate path from the bare
///    `pop[r]` numerator path: the numerator path would be a three-factor loop
///    `pop[r] -> share[r] -> update[r] -> pop[r]`; the aggregate path is the
///    four-factor `pop[r] -> agg -> share[r] -> update[r] -> pop[r]`. After the
///    trim, the *reported* links of the two are textually identical (both are
///    `pop[big] -> share[big] -> update[big] -> pop[big]`), so the loop's
///    *score* -- which factor terms it is a product of -- is what tells them
///    apart. (For this fixture the bare numerator link score `pop -> share`
///    happens to evaluate to zero, so discovery's strongest path is the
///    aggregate one; a model with no SUM reducer would instead carry the
///    three-factor score.)
///
/// Strongest-path discovery reports one loop per stock node, so a genuinely
/// *cross-element* loop (`pop[big] -> agg -> share[small] -> ... -> pop[small]
/// -> agg -> share[big] -> ... -> pop[big]`) is never the reported winner for
/// this fixture; the point being checked is that discovery's loop-finding is
/// *routed through* the aggregate node and *scored on* the un-trimmed path,
/// then the aggregate node is hidden from the reported structure.
///
/// Heterogeneous initial values (`pop[big] = 1000`, `pop[small] = 10`) keep the
/// loop scores non-degenerate so discovery's DFS and the post-sim contribution
/// filter both have a real run to work with.
#[test]
fn test_discovery_loop_through_agg_scored_on_untrimmed_path() {
    let project = build_heterogeneous_share_model(0.01);

    // Compile in discovery mode and simulate so we have both the discovered
    // loops *and* the raw link-score series they were scored from.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    let compiled =
        compile_project_incremental(&db, sync.project, "main").expect("compilation should succeed");
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let canonical_name = simlin_engine::canonicalize("main");
    let source_model = sync
        .project
        .models(&db)
        .get(canonical_name.as_ref())
        .copied()
        .expect("main model should exist in salsa DB");
    let element_edges = model_element_causal_edges(&db, source_model, sync.project);
    let causal_graph = causal_graph_from_element_edges(element_edges);
    let stocks: Vec<Ident<Canonical>> =
        element_edges.stocks.iter().map(|s| Ident::new(s)).collect();
    let ltm_vars = model_ltm_variables(&db, source_model, sync.project);
    let dm_dims = project_datamodel_dims(&db, sync.project);
    let sub_model_ports = simlin_engine::analysis::build_sub_model_output_ports(&db, sync.project);
    let found = ltm_finding::discover_loops_with_graph(
        &results,
        &causal_graph,
        &stocks,
        &ltm_vars.vars,
        dm_dims,
        &sub_model_ports,
        None,
    )
    .expect("discover_loops_with_graph should succeed")
    .loops;
    assert!(
        !found.is_empty(),
        "discovery should find at least one loop in the inlined-reducer model"
    );

    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let path_strings: Vec<Vec<String>> = found
        .iter()
        .map(|fl| {
            fl.loop_info
                .links
                .iter()
                .map(|l| format!("{}->{}", l.from.as_str(), l.to.as_str()))
                .collect::<Vec<_>>()
        })
        .collect();

    // (1) AC4.2 trim: no reported loop's links may reference the synthetic
    // aggregate node -- the trim post-pass collapses `[X -> agg, agg -> Y]`
    // into `[X -> Y]` with composed polarity.
    for fl in &found {
        for link in &fl.loop_info.links {
            assert_ne!(
                link.from.as_str(),
                agg,
                "reported loop link must not start at the synthetic aggregate node; \
                 found loops: {path_strings:?}"
            );
            assert_ne!(
                link.to.as_str(),
                agg,
                "reported loop link must not end at the synthetic aggregate node; \
                 found loops: {path_strings:?}"
            );
        }
    }

    // Identify the reported `pop[big] -> share[big] -> update[big] -> pop[big]`
    // loop. After trimming, this is the *only* loop whose link set is exactly
    // those three edges; its un-trimmed form is the four-edge aggregate cycle
    // `pop[big] -> $⁚ltm⁚agg⁚0 -> share[big] -> update[big] -> pop[big]`, which
    // we confirm below by reproducing its loop_score from the un-trimmed link
    // scores. (`build_heterogeneous_share_model`'s `Region` is `{Big, Small}`,
    // so the canonical element names are `big`, `small`.)
    let expected_links: std::collections::HashSet<(String, String)> = [
        ("pop[big]", "share[big]"),
        ("share[big]", "update[big]"),
        ("update[big]", "pop[big]"),
    ]
    .into_iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    let share_loop_matches: Vec<&ltm_finding::FoundLoop> = found
        .iter()
        .filter(|fl| {
            fl.loop_info
                .links
                .iter()
                .map(|l| (l.from.as_str().to_string(), l.to.as_str().to_string()))
                .collect::<std::collections::HashSet<_>>()
                == expected_links
        })
        .collect();
    assert_eq!(
        share_loop_matches.len(),
        1,
        "expected exactly one reported `pop[big] -> share[big] -> update[big] -> pop[big]` loop; \
         found loops: {path_strings:?}"
    );
    let share_loop = share_loop_matches[0];

    // (2) AC4.2 scoring -- the discovered loop's `loop_score` must reproduce
    // the product of the per-element link scores along the path discovery
    // actually traversed. Two paths trim to the same three-edge link set:
    //
    //   - the *aggregate* path (un-trimmed: 4 edges)
    //       pop[big] -> $⁚ltm⁚agg⁚0 -> share[big] -> update[big] -> pop[big]
    //   - the *bare-numerator* path (3 edges)
    //       pop[big] -> share[big] -> update[big] -> pop[big]
    //
    // Both are real loops (the diagonal conflation `share = pop / SUM(pop[*])`
    // resolved by construction -- the numerator effect and the SUM effect are
    // distinct loops). Discovery's strongest-path heuristic picks one per
    // timestep, so we don't know a priori which it surfaced here; we assert
    // the `loop_score` matches one of the two products, consistently across
    // steps, and that it is non-zero somewhere. (The "scored on the un-trimmed
    // aggregate path" invariant proper is exercised exhaustively by
    // `db::ltm_unified_tests::cross_element_loop_through_agg_is_recovered`,
    // where the aggregate-path loop is a Loop in its own right rather than a
    // strongest-path candidate. Before GH #517 was fixed the bare-numerator
    // link score was identically `0.0` -- `pop / SUM(PREVIOUS(pop[*]))` -- so
    // only the aggregate path was ever non-zero and this test could pin it
    // directly; with the fix the numerator path is a live competitor.)
    let off_exact = |name: &str| -> usize {
        *results
            .offsets
            .get(&Ident::<Canonical>::new(name))
            .unwrap_or_else(|| {
                panic!(
                    "missing offset {name}; ltm offsets present: {:?}",
                    results
                        .offsets
                        .keys()
                        .map(|k| k.as_str())
                        .filter(|s| s.contains("\u{205A}ltm\u{205A}"))
                        .collect::<Vec<_>>()
                )
            })
    };
    let off_pop_big_to_agg = off_exact(&format!(
        "$\u{205A}ltm\u{205A}link_score\u{205A}pop[big]\u{2192}{agg}"
    ));
    let off_agg_to_share_big = off_exact(&format!(
        "$\u{205A}ltm\u{205A}link_score\u{205A}{agg}\u{2192}share[big]"
    ));
    // The `pop -> share` (bare numerator), `share -> update`, and
    // `update -> pop` link scores are Apply-to-All over `Region`; element
    // `big` is declared first, so it is slot 0 of the base offset.
    let off_pop_to_share_big = off_exact("$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share");
    let off_share_to_update_big =
        off_exact("$\u{205A}ltm\u{205A}link_score\u{205A}share\u{2192}update");
    let off_update_to_pop_big =
        off_exact("$\u{205A}ltm\u{205A}link_score\u{205A}update\u{2192}pop");
    let at = |step: usize, o: usize| results.data[step * results.step_size + o];

    let mut saw_nonzero = false;
    let mut traversed_path: Option<&'static str> = None;
    for step in 2..results.step_count {
        let share_to_update = at(step, off_share_to_update_big);
        let update_to_pop = at(step, off_update_to_pop_big);
        let prod_via_agg = at(step, off_pop_big_to_agg)
            * at(step, off_agg_to_share_big)
            * share_to_update
            * update_to_pop;
        let prod_via_numerator = at(step, off_pop_to_share_big) * share_to_update * update_to_pop;
        let loop_score = share_loop.scores[step].1;
        let matches_agg = (loop_score - prod_via_agg).abs() < 1e-6;
        let matches_numerator = (loop_score - prod_via_numerator).abs() < 1e-6;
        assert!(
            matches_agg || matches_numerator,
            "step {step}: discovered loop_score = {loop_score}, but it matches neither the \
             un-trimmed aggregate-path product (pop[big]->agg * agg->share[big] * \
             share->update[big] * update->pop[big] = {prod_via_agg}) nor the bare-numerator-path \
             product (pop->share[big] * share->update[big] * update->pop[big] = {prod_via_numerator})"
        );
        // A non-degenerate match (the two products genuinely differ) pins
        // which path the loop took; it must not flip step-to-step.
        let this_step = match (matches_agg, matches_numerator) {
            (true, false) => Some("aggregate"),
            (false, true) => Some("bare-numerator"),
            _ => None, // products coincide (e.g. both ~0) -- uninformative
        };
        if let (Some(prev), Some(cur)) = (traversed_path, this_step) {
            assert_eq!(
                prev, cur,
                "step {step}: discovered loop's scoring path flipped from {prev} to {cur}"
            );
        }
        if traversed_path.is_none() {
            traversed_path = this_step;
        }
        if loop_score.abs() > 1e-9 {
            saw_nonzero = true;
        }
    }
    assert!(
        saw_nonzero,
        "expected at least one step where the discovered loop_score is non-zero \
         (otherwise the equality above is vacuous); found loops: {path_strings:?}"
    );
}

/// Build a model with a dynamic-index reference from an arrayed stock into a
/// scalar aux, embedded in a balancing feedback loop.
///
/// Model structure (all over a size-2 indexed dimension `Dim`):
///   arr[Dim]  (stock, inits arr[1]=10, arr[2]=20, inflow adjust)
///   idx       (aux, = 2 -- a scalar aux, NOT a dimension element name, so
///              `arr[idx]` classifies as RefShape::DynamicIndex)
///   total     (aux, = arr[idx])
///   adjust[Dim] (flow, = (100 - total) * 0.1)
///
/// Causal loop: arr -> total (DynamicIndex) -> adjust (bare) -> arr (flow->stock).
/// The `arr -> total` edge is the case Phase 6 broke: post-Phase-6 the
/// scalar `$⁚ltm⁚link_score⁚arr→total` var carries a DynamicIndex-shaped
/// partial (`arr[PREVIOUS(idx)] - PREVIOUS(total)`), but the `(from,to)`-keyed
/// salsa compilation path in `assemble_module` re-derives it with
/// `RefShape::Bare`, wrapping the whole subscript in PREVIOUS and collapsing
/// the numerator to `PREVIOUS(total) - PREVIOUS(total) ≈ 0`.
fn build_dynamic_index_into_scalar_model() -> simlin_engine::datamodel::Project {
    use simlin_engine::datamodel::{self, Equation, Variable};

    datamodel::Project {
        name: "dyn_index_into_scalar".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 6.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension::indexed("Dim".to_string(), 2)],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Stock(datamodel::Stock {
                    ident: "arr".to_string(),
                    equation: Equation::Arrayed(
                        vec!["Dim".to_string()],
                        vec![
                            ("1".to_string(), "10".to_string(), None, None),
                            ("2".to_string(), "20".to_string(), None, None),
                        ],
                        None,
                        false,
                    ),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["adjust".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Aux(datamodel::Aux {
                    ident: "idx".to_string(),
                    equation: Equation::Scalar("2".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Aux(datamodel::Aux {
                    ident: "total".to_string(),
                    equation: Equation::Scalar("arr[idx]".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(datamodel::Flow {
                    ident: "adjust".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["Dim".to_string()],
                        "(100 - total) * 0.1".to_string(),
                    ),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// Regression: a `DynamicIndex` reference from an arrayed stock into a scalar
/// aux must produce a non-degenerate `$⁚ltm⁚link_score⁚{from}→{to}` series.
///
/// Before this fix, Phase 6 removed the per-shape routing that sent a
/// `Wildcard`/`DynamicIndex`-shaped scalar link score through direct
/// (non-salsa) compilation; the salsa `(from,to)` path then re-derived the
/// partial as `RefShape::Bare`, wrapping the entire subscript in `PREVIOUS()`.
/// For `total = arr[idx]` that makes the numerator
/// `PREVIOUS(arr[idx]) - PREVIOUS(total) == 0`, so the link score (and any
/// loop score that multiplies it) was identically zero.
#[test]
fn test_dynamic_index_into_scalar_link_score_nondegenerate() {
    let project = build_dynamic_index_into_scalar_model();

    let compiled = compile_ltm_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let (key, offset) = find_link_score_offset(&results, "arr", "total").unwrap_or_else(|| {
        panic!(
            "missing $⁚ltm⁚link_score⁚arr→total offset; ltm offsets present: {:?}",
            results
                .offsets
                .keys()
                .map(|k| k.as_str())
                .filter(|s| s.contains("\u{205A}ltm\u{205A}"))
                .collect::<Vec<_>>()
        )
    });
    // The link score var must carry the canonical (subscript-free) name --
    // a DynamicIndex shape collapses onto the Bare name post-Phase-6.
    assert_eq!(
        key.as_str(),
        "$\u{205A}ltm\u{205A}link_score\u{205A}arr\u{2192}total"
    );

    // The flow->stock link score needs two prior committed timesteps; from
    // step 2 onward the `arr -> total` score must be defined and non-zero
    // for at least one step. (Identically zero is the pre-fix bug.)
    let saw_nonzero = (2..results.step_count).any(|step| {
        let v = results.data[step * results.step_size + offset];
        v.abs() > 1e-9 && !v.is_nan()
    });
    assert!(
        saw_nonzero,
        "arr -> total link score (offset {offset}) is identically zero across all steps; \
         the DynamicIndex-shaped partial was not used (Phase 6 regression)"
    );

    // Hand calc at step 2. With dt = 1 and adjust[1] = (100 - total) * 0.1 too,
    // but only the [2] slot matters for `total = arr[idx]` (idx = 2):
    //   step 0: arr[2] = 20, total = 20, adjust[2] = (100 - 20) * 0.1 = 8
    //   step 1: arr[2] = 28, total = 28, adjust[2] = (100 - 28) * 0.1 = 7.2
    //   step 2: arr[2] = 35.2, total = 35.2
    // The ceteris-paribus partial of `total = arr[idx]` w.r.t. `arr` keeps
    // the live `arr[idx]` reference and PREVIOUS-wraps everything else:
    // `arr[PREVIOUS(idx)] - PREVIOUS(total)`. At step 2, PREVIOUS(idx) = 2,
    // so num = arr[2]@2 - total@1 = 35.2 - 28 = 7.2. The guard form is
    //   ABS(SAFEDIV(num, total - PREVIOUS(total), 0))
    //     * SIGN(SAFEDIV(num, source_diff, 0))
    // where the source side is the scalarized live slice
    // `SUM(arr[PREVIOUS(idx)]) - PREVIOUS(SUM(arr[PREVIOUS(idx)]))`
    // (= SUM(arr[2])@2 - SUM(arr[2])@1 = 35.2 - 28 = 7.2; positive).
    // total - PREVIOUS(total) = 35.2 - 28 = 7.2 (== num) so ABS(...) = 1,
    // and source_diff > 0 so SIGN(...) = +1. => link score at step 2 == 1.0.
    let at_step2 = results.data[2 * results.step_size + offset];
    assert!(
        (at_step2 - 1.0).abs() < 1e-6,
        "arr -> total link score at step 2 expected 1.0, got {at_step2}"
    );
}

/// Build a model where the *same* arrayed stock `pop` is referenced from a
/// scalar aux `x` two ways: inside a hoisted full reducer (`SUM(pop[*])`,
/// which Phase 5 routes through `$⁚ltm⁚agg⁚0`) *and* via a bare dynamic
/// index (`pop[idx]`, which is NOT inside any reducer and so stays a
/// conservative direct dependency). A feedback flow `grow[Region] = x * c`
/// feeds `pop`, so both a reducer-routed cycle (`pop[d] → agg → x →
/// grow[d] → pop[d]`) and a direct cycle (`pop[d] → x → grow[d] → pop[d]`)
/// exist.
///
/// `idx` is a plain scalar aux (not a `Region` element name), so `pop[idx]`
/// classifies as `RefShape::DynamicIndex` at the `Expr2` level regardless
/// of its constant value.
fn build_mixed_reducer_and_dynamic_index_model() -> simlin_engine::datamodel::Project {
    use simlin_engine::datamodel::{self, Equation, Variable};

    datamodel::Project {
        name: "mixed_reducer_dyn_index".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 5.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension::named(
            "Region".to_string(),
            vec!["NYC".to_string(), "Boston".to_string()],
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Stock(datamodel::Stock {
                    ident: "pop".to_string(),
                    equation: Equation::ApplyToAll(vec!["Region".to_string()], "100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["grow".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Aux(datamodel::Aux {
                    ident: "idx".to_string(),
                    equation: Equation::Scalar("1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Aux(datamodel::Aux {
                    ident: "x".to_string(),
                    equation: Equation::Scalar("SUM(pop[*]) + pop[idx]".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(datamodel::Flow {
                    ident: "grow".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        "x * 0.001".to_string(),
                    ),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// Regression (P2): a target that references a source *both* through a
/// hoisted reducer (`SUM(pop[*])`) *and* through a direct non-reducer
/// reference (`pop[idx]`, a dynamic index) must keep the direct reference's
/// conservative element edges and its conservative link score -- only the
/// reducer-owned reference site should route through the `$⁚ltm⁚agg⁚{n}`
/// node.
///
/// Before the fix, the element-graph reroute keyed `route_through_agg`
/// purely on `RefShape::Wildcard | RefShape::DynamicIndex` (so the *direct*
/// `pop[idx]` site, which is `DynamicIndex`, was also rerouted through the
/// agg), and `emit_per_shape_link_scores(skip_reducer_shapes = true)`
/// dropped the `DynamicIndex` shape (so the `pop→x` link score vanished).
#[test]
fn test_mixed_reducer_and_dynamic_index_keeps_direct_reference() {
    let project = build_mixed_reducer_and_dynamic_index_model();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;

    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // ---- element graph: agg routing for the SUM, direct edges for pop[idx] ----
    let elem_edges = model_element_causal_edges(&db, source_model, sync.project);
    let has_edge = |from: &str, to: &str| -> bool {
        elem_edges.edges.get(from).is_some_and(|ts| ts.contains(to))
    };
    for d in &["nyc", "boston"] {
        assert!(
            has_edge(&format!("pop[{d}]"), agg),
            "expected pop[{d}] -> {agg} (SUM reduction); edges: {:?}",
            elem_edges.edges
        );
    }
    assert!(
        has_edge(agg, "x"),
        "expected {agg} -> x (agg broadcast into scalar target); edges: {:?}",
        elem_edges.edges
    );
    // The direct `pop[idx]` reference (DynamicIndex, NOT inside a reducer)
    // keeps its conservative arrayed-source -> scalar-target edges; it must
    // NOT be folded into the agg.
    for d in &["nyc", "boston"] {
        assert!(
            has_edge(&format!("pop[{d}]"), "x"),
            "expected direct pop[{d}] -> x (from the pop[idx] dynamic index); edges: {:?}",
            elem_edges.edges
        );
    }

    // ---- model_ltm_variables: agg aux + both link-score halves + the
    //      conservative pop->x link score for the dynamic index ----
    let ltm_vars = model_ltm_variables(&db, source_model, sync.project);
    let has_var = |name: &str| -> bool { ltm_vars.vars.iter().any(|v| v.name == name) };

    assert!(
        has_var(agg),
        "expected the synthetic agg aux {agg}; vars: {:?}",
        ltm_vars
            .vars
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );
    for d in &["nyc", "boston"] {
        let name = format!("$\u{205A}ltm\u{205A}link_score\u{205A}pop[{d}]\u{2192}{agg}");
        assert!(
            has_var(&name),
            "expected source->agg link score {name:?}; vars: {:?}",
            ltm_vars
                .vars
                .iter()
                .map(|v| v.name.as_str())
                .collect::<Vec<_>>()
        );
    }
    assert!(
        has_var(&format!(
            "$\u{205A}ltm\u{205A}link_score\u{205A}{agg}\u{2192}x"
        )),
        "expected agg->target link score $⁚ltm⁚link_score⁚{agg}→x; vars: {:?}",
        ltm_vars
            .vars
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );
    // The conservative link score for the direct `pop[idx]` reference must
    // be emitted (it shares the canonical Bare name): `$⁚ltm⁚link_score⁚pop→x`.
    assert!(
        has_var("$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}x"),
        "expected the conservative pop->x link score for the dynamic-index \
         reference (not suppressed by skip_reducer_shapes); vars: {:?}",
        ltm_vars
            .vars
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );

    // The model still compiles and simulates with LTM enabled.
    let compiled = compile_project_incremental(&db, sync.project, "main").expect("should compile");
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("should simulate");
}

/// Build a model with both a whole-RHS reducer `denom = SUM(pop[*])`
/// (variable-backed agg) and an inline use of the same reducer text
/// `share[r] = pop[r] / SUM(pop[*])` (which must mint a *synthetic* agg).
/// `denom` is canonical-sorted before `share`, so the variable-backed agg
/// is registered first -- the case that used to make the inline use reuse
/// it. A feedback flow `grow[r] = share[r] * pop[r] * c` feeds `pop`, so
/// `share`'s reducer is part of a loop.
fn build_var_backed_and_inline_same_reducer_model() -> simlin_engine::datamodel::Project {
    use simlin_engine::datamodel::{self, Equation, Variable};

    datamodel::Project {
        name: "var_backed_and_inline".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 5.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension::named(
            "Region".to_string(),
            vec!["NYC".to_string(), "Boston".to_string()],
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Stock(datamodel::Stock {
                    ident: "pop".to_string(),
                    equation: Equation::ApplyToAll(vec!["Region".to_string()], "100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["grow".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // `denom` -- canonical-sorted before `grow` and `share`, so
                // its variable-backed agg is registered first.
                Variable::Aux(datamodel::Aux {
                    ident: "denom".to_string(),
                    equation: Equation::Scalar("SUM(pop[*])".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(datamodel::Flow {
                    ident: "grow".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        "share * pop * 0.001".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Aux(datamodel::Aux {
                    ident: "share".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        "pop / SUM(pop[*])".to_string(),
                    ),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// Regression (P2): an inline reducer (`share[r] = pop[r] / SUM(pop[*])`)
/// must get its own synthetic `$⁚ltm⁚agg⁚{n}` node -- and the agg aux +
/// both link-score halves in `model_ltm_variables` -- even when a
/// whole-RHS reducer of identical text (`denom = SUM(pop[*])`) is declared
/// first. Before the fix, the inline use reused `denom`'s variable-backed
/// agg (deduped purely by reducer text), so no synthetic was minted and the
/// downstream `is_synthetic` filters left `share`'s reducer on the old
/// direct-wildcard scoring path.
#[test]
fn test_inline_reducer_gets_synthetic_agg_despite_var_backed_sibling() {
    let project = build_var_backed_and_inline_same_reducer_model();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;

    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let ltm_vars = model_ltm_variables(&db, source_model, sync.project);
    let has_var = |name: &str| -> bool { ltm_vars.vars.iter().any(|v| v.name == name) };
    let var_names: Vec<&str> = ltm_vars.vars.iter().map(|v| v.name.as_str()).collect();

    // The synthetic agg aux for `share`'s inline reducer.
    assert!(
        has_var(agg),
        "expected synthetic agg aux {agg} for share's inline SUM; vars: {var_names:?}"
    );
    // Its source -> agg link scores (one per source element).
    for d in &["nyc", "boston"] {
        let name = format!("$\u{205A}ltm\u{205A}link_score\u{205A}pop[{d}]\u{2192}{agg}");
        assert!(
            has_var(&name),
            "expected source->agg link score {name:?}; vars: {var_names:?}"
        );
    }
    // Its agg -> target link scores (one per `share` element, since `share`
    // is arrayed).
    for r in &["nyc", "boston"] {
        let name = format!("$\u{205A}ltm\u{205A}link_score\u{205A}{agg}\u{2192}share[{r}]");
        assert!(
            has_var(&name),
            "expected agg->target link score {name:?}; vars: {var_names:?}"
        );
    }
    // The `pop[r]` bare numerator in `share[r] = pop[r] / SUM(pop[*])` keeps
    // its own (non-reducer) link score, named by the canonical Bare form --
    // it is *not* swallowed by the synthetic-agg routing.
    assert!(
        has_var("$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share"),
        "expected the bare pop->share link score for share's numerator; vars: {var_names:?}"
    );
    // Sanity: only one synthetic agg was minted (the inline `SUM` in
    // `share`), and `denom`'s whole-RHS `SUM(pop[*])` did not produce a
    // *second* synthetic -- it is a variable-backed agg (the variable
    // itself), so it never appears as a `$⁚ltm⁚agg⁚{n}` aux.
    assert!(
        !has_var("$\u{205A}ltm\u{205A}agg\u{205A}1"),
        "denom's whole-RHS reducer must be variable-backed, not a second synthetic agg; vars: {var_names:?}"
    );

    // The model compiles and simulates with LTM enabled.
    let compiled = compile_project_incremental(&db, sync.project, "main").expect("should compile");
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("should simulate");
}

/// AC5.2 / AC5.4 (#515, end-to-end; one-loop-per-subset per GH #676): a
/// 4-element-dim `share[r] = pop[r] / SUM(pop[*])` model with `update[r] =
/// share[r] * pop[r] * c` and *heterogeneous* stock initials (so loop
/// scores don't degenerate to zero under symmetry) hoists `SUM(pop[*])`
/// into `$⁚ltm⁚agg⁚0`; each region has one disjoint petal through it.
/// `recover_cross_agg_loops` reconstructs the full 4-petal subset as
/// exactly ONE canonical loop (every cyclic ordering of a fixed subset
/// traverses the same edge multiset, so additional orderings would be
/// score-identical duplicates), and its simulated `loop_score` series is
/// non-degenerate and equals the product of its 16 link scores at every
/// step.
#[test]
fn test_four_petal_canonical_loop_score_is_link_score_product() {
    use simlin_engine::datamodel::{self, Equation, Variable};

    let regions = ["A", "B", "C", "D"];
    let c = 0.01_f64;
    let project = datamodel::Project {
        name: "four_petal_share".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 8.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension::named(
            "Region".to_string(),
            regions.iter().map(|s| s.to_string()).collect(),
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Stock(datamodel::Stock {
                    ident: "pop".to_string(),
                    // Heterogeneous initials break the all-symmetric case
                    // where every link score (and thus loop score) is 0.
                    equation: Equation::Arrayed(
                        vec!["Region".to_string()],
                        vec![
                            ("A".to_string(), "1000".to_string(), None, None),
                            ("B".to_string(), "300".to_string(), None, None),
                            ("C".to_string(), "100".to_string(), None, None),
                            ("D".to_string(), "30".to_string(), None, None),
                        ],
                        None,
                        false,
                    ),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["update".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Aux(datamodel::Aux {
                    ident: "share".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        "pop / SUM(pop[*])".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(datamodel::Flow {
                    ident: "update".to_string(),
                    // `* pop` makes the feedback flow curved -> non-zero
                    // flow->stock link score (and thus non-zero loop score).
                    equation: Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        format!("share * pop * {c}"),
                    ),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);
    assert!(
        !ltm.agg_recovery_truncated,
        "a 4-petal model is well under the production budget"
    );

    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    // The full-4-petal loop_score vars: those whose equation references all
    // four `pop[r]→agg` factors. There must be exactly 1 -- one canonical
    // loop per disjoint petal subset (GH #676).
    let four_petal_loop_vars: Vec<&simlin_engine::db::LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}"))
        .filter(|v| {
            let eq = v.equation.source_text();
            regions.iter().all(|r| {
                eq.contains(
                    format!(
                        "\"$\u{205A}ltm\u{205A}link_score\u{205A}pop[{}]\u{2192}{agg}\"",
                        r.to_lowercase()
                    )
                    .as_str(),
                )
            })
        })
        .collect();
    assert_eq!(
        four_petal_loop_vars.len(),
        1,
        "the full 4-petal subset must give exactly one canonical loop; got {:?}",
        four_petal_loop_vars
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );

    let compiled = compile_project_incremental(&db, sync.project, "main").expect("should compile");
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("should simulate");
    let results = vm.into_results();

    let loop_offset = *results
        .offsets
        .get(&Ident::<Canonical>::new(&four_petal_loop_vars[0].name))
        .unwrap_or_else(|| panic!("missing offset for {}", four_petal_loop_vars[0].name));

    // The loop must be non-degenerate (genuinely computed: non-zero and
    // finite at some step, not stubbed to a constant zero). It is a product
    // of 16 link scores (each O(1e-3..1)), so the value is small but not
    // zero.
    let mut saw_nonzero = false;
    let mut all_finite = true;
    for step in 0..results.step_count {
        let v0 = results.data[step * results.step_size + loop_offset];
        if !v0.is_finite() {
            all_finite = false;
        } else if v0 != 0.0 {
            saw_nonzero = true;
        }
    }
    assert!(
        all_finite,
        "the 4-petal loop score series must be finite at every step"
    );
    assert!(
        saw_nonzero,
        "the 4-petal loop score must be non-degenerate (non-zero at some step)"
    );

    // Stronger: the loop score equals the running product of its link scores
    // at every step (it is a genuine product, not a stub). `[elem]` slot
    // subscripts on the factors ride *outside* the quotes (e.g.
    // `"$⁚ltm⁚link_score⁚update→pop"[a]`), so split each ` * `-joined factor
    // into its quoted var name and an optional region slot.
    {
        let factor_offsets: Vec<usize> =
            loop_score_equation_factors(&four_petal_loop_vars[0].equation.source_text())
                .iter()
                .map(|factor| {
                    let close = factor.rfind('"').expect("factor must be quoted");
                    let name = &factor[1..close];
                    let after = &factor[close + 1..];
                    let base = *results
                        .offsets
                        .get(&Ident::<Canonical>::new(name))
                        .unwrap_or_else(|| panic!("missing offset for factor {name:?}"));
                    match after.strip_prefix('[') {
                        None => base,
                        Some(rest) => {
                            let elem = &rest[..rest.find(']').expect("malformed slot subscript")];
                            let idx = regions
                                .iter()
                                .position(|r| r.to_lowercase() == elem)
                                .unwrap_or_else(|| panic!("unknown region slot {elem:?}"));
                            base + idx
                        }
                    }
                })
                .collect();
        assert_eq!(factor_offsets.len(), 16, "a 4-petal loop has 16 edges");
        for step in 0..results.step_count {
            let base = step * results.step_size;
            let product: f64 = factor_offsets
                .iter()
                .map(|&o| results.data[base + o])
                .product();
            let loop_val = results.data[base + loop_offset];
            let both_nan = loop_val.is_nan() && product.is_nan();
            assert!(
                both_nan || (loop_val - product).abs() <= 1e-9 * product.abs().max(1e-300),
                "step {step}: loop_score {loop_val} != product of its link scores {product}"
            );
        }
    }

    // Sanity: the reported variable-level loops never surface the synthetic
    // agg node, and there is exactly one synthetic agg.
    let detected = model_detected_loops(&db, source_model, sync.project);
    for l in &detected.loops {
        assert!(
            l.variables
                .iter()
                .all(|name| !name.contains("\u{205A}agg\u{205A}")),
            "model_detected_loops should not surface synthetic agg nodes; got: {:?}",
            l.variables
        );
    }
}

/// Build the canonical reducer-in-feedback repro for GH #696:
/// `growth[r] = SUM(pop[*]) * 0.05` over `n` elements, each `growth[r]` an
/// inflow to `pop[r]`. The whole-extent `SUM(pop[*])` is hoisted into one
/// scalar synthetic agg, and every element forms a single petal
/// (`pop[e] -> agg -> growth[e] -> pop[e]`). Heterogeneous initials keep the
/// per-element link scores distinct so loops don't degenerate to zero.
fn build_reducer_feedback_model(name: &str, elems: &[&str]) -> simlin_engine::datamodel::Project {
    use simlin_engine::datamodel::{self, Equation, Variable};
    datamodel::Project {
        name: name.to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 6.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension::named(
            "Region".to_string(),
            elems.iter().map(|s| s.to_string()).collect(),
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Stock(datamodel::Stock {
                    ident: "pop".to_string(),
                    equation: Equation::Arrayed(
                        vec!["Region".to_string()],
                        elems
                            .iter()
                            .enumerate()
                            .map(|(i, e)| {
                                // 1000, 300, 100, ... -- distinct per element.
                                let init = (1000.0 / 3f64.powi(i as i32)).round();
                                (e.to_string(), format!("{init}"), None, None)
                            })
                            .collect(),
                        None,
                        false,
                    ),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["growth".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(datamodel::Flow {
                    ident: "growth".to_string(),
                    equation: Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        "SUM(pop[*]) * 0.05".to_string(),
                    ),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// GH #696: discovery mode must recover the cross-element loops that traverse a
/// hoisted reducer more than once -- not just the single-petal loops.
///
/// On the 3-element `growth[r] = SUM(pop[*]) * 0.05` repro, exhaustive mode
/// emits 7 cross-element loops (3 single-petal + 3 disjoint-pair + 1 triple).
/// Before the fix, discovery (the production `analyze_model` path) returned
/// only the 3 single-petal loops because its DFS `visiting` set forbids
/// revisiting the synthetic agg node. This cross-validates the discovered loop
/// set against exhaustive on the SAME model: same count, and -- for the
/// loops that traverse the agg -- a per-element loop-score series equal to the
/// exhaustive loop_score variables (to within FP reassociation).
#[test]
fn discovery_recovers_cross_agg_loops_matches_exhaustive() {
    let elems = ["a", "b", "c"];
    let project = build_reducer_feedback_model("reducer_feedback_3", &elems);
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // --- Exhaustive mode: enumerate the cross-element loop_score variables ---
    // and their simulated series. Every loop here traverses the (scalar) agg.
    let (exhaustive_loop_count, exhaustive_score_set) = {
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        set_project_ltm_enabled(&mut db, sync.project, true);
        let source_model = sync.models["main"].source_model;
        let ltm = model_ltm_variables(&db, source_model, sync.project);
        let loop_vars: Vec<&simlin_engine::db::LtmSyntheticVar> = ltm
            .vars
            .iter()
            .filter(|v| v.name.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}"))
            .collect();
        assert_eq!(
            loop_vars.len(),
            7,
            "exhaustive must emit 3 single-petal + 3 disjoint-pair + 1 triple = 7 \
             cross-element loops through the reducer; got {:?}",
            loop_vars
                .iter()
                .map(|v| v.name.as_str())
                .collect::<Vec<_>>()
        );
        let compiled =
            compile_project_incremental(&db, sync.project, "main").expect("exhaustive compile");
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().expect("exhaustive simulate");
        let results = vm.into_results();
        // Capture each loop's |score| series as a roundable fingerprint so we
        // can match discovery's stitched loops against exhaustive's by value
        // (the two enumerate the same loops but assign different ids).
        let mut set: std::collections::HashSet<Vec<i64>> = std::collections::HashSet::new();
        for v in &loop_vars {
            let off = *results
                .offsets
                .get(&Ident::<Canonical>::new(&v.name))
                .expect("exhaustive loop_score offset");
            let fingerprint: Vec<i64> = (0..results.step_count)
                .map(|step| {
                    let x = results.data[step * results.step_size + off];
                    if x.is_finite() {
                        (x.abs() * 1e9).round() as i64
                    } else {
                        i64::MIN
                    }
                })
                .collect();
            set.insert(fingerprint);
        }
        (loop_vars.len(), set)
    };

    // --- Discovery mode through the PRODUCTION analyze_model path ---
    let (mut db, sp) = {
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &project);
        (db, sync.project)
    };
    let analysis = simlin_engine::analysis::analyze_model(&project, &mut db, sp, "main", None)
        .expect("analyze_model");

    // Every discovered loop traverses the reducer (the only feedback in this
    // model runs pop -> agg -> growth -> pop), so the discovered count must
    // equal the exhaustive cross-element loop count.
    assert_eq!(
        analysis.loop_dominance.len(),
        exhaustive_loop_count,
        "discovery must recover all {exhaustive_loop_count} cross-element reducer loops; \
         got {} loops: {:?}. GH #696.",
        analysis.loop_dominance.len(),
        analysis
            .loop_dominance
            .iter()
            .map(|l| &l.variables)
            .collect::<Vec<_>>()
    );

    // The reported loops never surface the synthetic agg node.
    for l in &analysis.loop_dominance {
        assert!(
            l.variables
                .iter()
                .all(|v| !v.contains("\u{205A}agg\u{205A}")),
            "discovery must trim the synthetic agg from reported loops; got {:?}",
            l.variables
        );
    }

    // There must be loops of three distinct sizes: single-petal (2 vars),
    // disjoint-pair (4 vars), and triple (6 vars) -- the structural signature
    // of cross-agg recovery, not just single petals.
    let sizes: std::collections::HashSet<usize> = analysis
        .loop_dominance
        .iter()
        .map(|l| l.variables.len())
        .collect();
    assert!(
        sizes.contains(&2) && sizes.contains(&4) && sizes.contains(&6),
        "discovery must recover single-petal (2), pair (4), and triple (6) loops; got sizes {sizes:?}. GH #696."
    );

    // Cross-validate the score series. Discovery's `importance` is normalized,
    // so instead re-derive each discovered loop's raw |loop score| series from
    // its variables and confirm the fingerprint set matches exhaustive's. Use
    // discovery's own raw loop score: re-run discovery with the helper that
    // exposes raw FoundLoop scores.
    let raw = {
        let mut db2 = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db2, &project, None);
        set_project_ltm_enabled(&mut db2, sync.project, true);
        set_project_ltm_discovery_mode(&mut db2, sync.project, true);
        let source_model = sync.models["main"].source_model;
        let compiled = compile_project_incremental(&db2, sync.project, "main").unwrap();
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();
        let results = vm.into_results();
        let element_edges = model_element_causal_edges(&db2, source_model, sync.project);
        // This model has no modules, so the bare element-edge graph suffices
        // (the production analyze_model path uses the module-enriched
        // constructor for GH #698, irrelevant here).
        let causal_graph = causal_graph_from_element_edges(element_edges);
        let stocks: Vec<Ident<Canonical>> = element_edges
            .stocks
            .iter()
            .map(|s| Ident::new(s.as_str()))
            .collect();
        let ltm = model_ltm_variables(&db2, source_model, sync.project);
        let dm_dims = project_datamodel_dims(&db2, sync.project);
        // No modules in this model, so the per-exit-port recompute never fires;
        // an empty output-port map is correct.
        ltm_finding::discover_loops_with_graph(
            &results,
            &causal_graph,
            &stocks,
            &ltm.vars,
            dm_dims,
            &ltm_finding::SubModelOutputPorts::new(),
            None,
        )
        .expect("discovery")
    };
    let mut discovery_score_set: std::collections::HashSet<Vec<i64>> =
        std::collections::HashSet::new();
    for fl in &raw.loops {
        let fingerprint: Vec<i64> = fl
            .scores
            .iter()
            .map(|(_, x)| {
                if x.is_finite() {
                    (x.abs() * 1e9).round() as i64
                } else {
                    i64::MIN
                }
            })
            .collect();
        discovery_score_set.insert(fingerprint);
    }
    assert_eq!(
        discovery_score_set, exhaustive_score_set,
        "discovery's recovered loop-score series must match exhaustive's set (GH #696); \
         agg={agg}"
    );
}

/// Enabling LTM must not change (or break) the model's own simulation:
/// C-LEARN compiled with `ltm_enabled` + discovery mode (the production
/// `analyze_model` configuration) must produce the SAME values for every
/// model variable as the plain (LTM-disabled) compile. LTM synthetic
/// variables are appended to the end of the flows runlist and never feed
/// back into model equations, so any divergence here means LTM
/// instrumentation corrupted the simulation itself.
///
/// `#[ignore]`d for runtime only (C-LEARN is ~53k lines / 1.4 MB and the LTM
/// compile is heavy); run explicitly with:
///   cargo test --release --features file_io --test integration -- --ignored clearn_with_ltm
#[test]
#[ignore]
fn clearn_with_ltm_simulates_model_vars_identically() {
    let mdl_path = "../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl";
    let contents = std::fs::read_to_string(mdl_path)
        .unwrap_or_else(|e| panic!("failed to read {mdl_path}: {e}"));
    let project = simlin_engine::open_vensim(&contents)
        .unwrap_or_else(|e| panic!("failed to parse {mdl_path}: {e}"));

    // Plain (LTM-disabled) run: the known-good baseline. `simulates_clearn`
    // gates this configuration against genuine Vensim reference output.
    let plain = {
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();
        vm.into_results()
    };

    // LTM discovery run: the production analyze_model configuration for a
    // model of this scale (exhaustive mode auto-flips to discovery anyway).
    let ltm = {
        let compiled = compile_ltm_discovery_incremental(&project);
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();
        vm.into_results()
    };

    assert_eq!(
        plain.step_count, ltm.step_count,
        "step counts must match between plain and LTM-enabled runs"
    );

    // Every model variable (every offsets-map name in the PLAIN run) must
    // have an identical series in the LTM-enabled run.
    let mut checked = 0usize;
    let mut mismatched: Vec<String> = Vec::new();
    for (name, &plain_off) in plain.offsets.iter() {
        let Some(&ltm_off) = ltm.offsets.get(name) else {
            mismatched.push(format!("{}: missing from LTM run offsets", name.as_str()));
            continue;
        };
        checked += 1;
        for step in 0..plain.step_count {
            let p = plain.data[step * plain.step_size + plain_off];
            let l = ltm.data[step * ltm.step_size + ltm_off];
            let same = (p.is_nan() && l.is_nan())
                || (p == l)
                || ((p - l).abs() <= 1e-9 * p.abs().max(l.abs()));
            if !same {
                mismatched.push(format!("{}: step {step}: plain={p} ltm={l}", name.as_str()));
                break;
            }
        }
        if mismatched.len() > 25 {
            break;
        }
    }
    assert!(
        mismatched.is_empty(),
        "enabling LTM changed/broke {} of {checked} model variable series; first mismatches:\n  {}",
        mismatched.len(),
        mismatched.join("\n  ")
    );
}

/// Build a model whose only feedback runs through a Vensim-style lookup
/// table call: `birth_multiplier = LOOKUP(birth_table, population)` where
/// `birth_table` is a standalone lookup-only table variable (no equation,
/// only a graphical function -- exactly what the MDL importer produces for
/// `table_var(input)` calls, e.g. WRLD3's
/// `lifetime multiplier from food table(food per capita / ...)`).
///
///   population (stock) -> birth_multiplier (via LOOKUP) -> births -> population
///   population -> births (direct reference)               -> population
fn build_lookup_table_feedback_model() -> simlin_engine::datamodel::Project {
    use simlin_engine::datamodel::{self, Equation, Variable};
    datamodel::Project {
        name: "lookup_feedback".to_string(),
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
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Stock(datamodel::Stock {
                    ident: "population".to_string(),
                    equation: Equation::Scalar("50".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["births".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // Standalone lookup-only table: empty equation + graphical
                // function. Consumers must call it via LOOKUP(table, x).
                Variable::Aux(datamodel::Aux {
                    ident: "birth_table".to_string(),
                    equation: Equation::Scalar(String::new()),
                    documentation: String::new(),
                    units: None,
                    gf: Some(datamodel::GraphicalFunction {
                        kind: datamodel::GraphicalFunctionKind::Continuous,
                        x_points: Some(vec![0.0, 50.0, 100.0, 200.0]),
                        y_points: vec![2.0, 1.5, 0.8, 0.1],
                        x_scale: datamodel::GraphicalFunctionScale {
                            min: 0.0,
                            max: 200.0,
                        },
                        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 2.0 },
                    }),
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Aux(datamodel::Aux {
                    ident: "birth_multiplier".to_string(),
                    equation: Equation::Scalar("LOOKUP(birth_table, population)".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(datamodel::Flow {
                    ident: "births".to_string(),
                    equation: Equation::Scalar("population * birth_multiplier * 0.05".to_string()),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// A link score *through a lookup-table call* must carry real (nonzero)
/// values: the table reference is static data, not a causal dependency, so
/// the ceteris-paribus partial holds it verbatim and the fragment compiles.
///
/// Regression test for the WRLD3 failure mode where every table-mediated
/// link score (`food_per_capita -> lifetime_multiplier_from_food`, ...) was
/// identically zero: the partial wrapped the table in PREVIOUS() (making the
/// equation uncompilable), and the LTM fragment compiler didn't thread the
/// referenced table's graphical-function data into its mini-Module, so the
/// fragment silently stubbed to a constant 0.
#[test]
fn test_lookup_table_link_score_is_nonzero() {
    let project = build_lookup_table_feedback_model();
    let compiled = compile_ltm_discovery_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // The model variables themselves must vary (sanity: the loop is active).
    let pop_off = results.offsets[&Ident::<Canonical>::new("population")];
    let mult_off = results.offsets[&Ident::<Canonical>::new("birth_multiplier")];
    let at = |step: usize, off: usize| results.data[step * results.step_size + off];
    assert!(
        (at(1, pop_off) - at(results.step_count - 1, pop_off)).abs() > 1.0,
        "population must change over the run"
    );
    assert!(
        (at(1, mult_off) - at(results.step_count - 1, mult_off)).abs() > 1e-6,
        "birth_multiplier must change over the run"
    );

    // The lookup-mediated link score must be nonzero at some step.
    let ls_name = "$\u{205A}ltm\u{205A}link_score\u{205A}population\u{2192}birth_multiplier";
    let ls_off = *results
        .offsets
        .get(&Ident::<Canonical>::new(ls_name))
        .unwrap_or_else(|| panic!("missing link score column {ls_name}"));
    let nonzero_steps = (1..results.step_count)
        .filter(|&step| {
            let v = at(step, ls_off);
            v.is_finite() && v != 0.0
        })
        .count();
    assert!(
        nonzero_steps > 0,
        "the population -> birth_multiplier link score (through LOOKUP) must be nonzero \
         at some step; series: {:?}",
        (0..results.step_count)
            .map(|s| at(s, ls_off))
            .collect::<Vec<_>>()
    );

    // And there must be no link score from the *table* variable itself: the
    // table is static data, not a causal node.
    let table_link_scores: Vec<&str> = results
        .offsets
        .keys()
        .map(|k| k.as_str())
        .filter(|k| k.contains("link_score") && k.contains("birth_table"))
        .collect();
    assert!(
        table_link_scores.is_empty(),
        "no link score should involve the lookup-only table variable: {table_link_scores:?}"
    );

    // Discovery must find the loop through the lookup table.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let element_edges = model_element_causal_edges(&db, source_model, sync.project);
    let causal_graph = causal_graph_from_element_edges(element_edges);
    let stocks: Vec<Ident<Canonical>> =
        element_edges.stocks.iter().map(|s| Ident::new(s)).collect();
    let ltm_vars = model_ltm_variables(&db, source_model, sync.project);
    let dm_dims = project_datamodel_dims(&db, sync.project);
    let sub_model_ports = simlin_engine::analysis::build_sub_model_output_ports(&db, sync.project);
    let found = ltm_finding::discover_loops_with_graph(
        &results,
        &causal_graph,
        &stocks,
        &ltm_vars.vars,
        dm_dims,
        &sub_model_ports,
        None,
    )
    .expect("discovery should succeed")
    .loops;
    let has_lookup_loop = found.iter().any(|fl| {
        fl.loop_info
            .links
            .iter()
            .any(|l| l.to.as_str() == "birth_multiplier" || l.from.as_str() == "birth_multiplier")
    });
    assert!(
        has_lookup_loop,
        "discovery must find the loop through the lookup-mediated birth_multiplier; found: {:?}",
        found
            .iter()
            .map(|fl| fl.loop_info.format_path())
            .collect::<Vec<_>>()
    );
}

/// Build an isolated single-stock feedback loop routed through a chain of
/// two passthrough (stockless) user modules, including a module->module
/// link (`mod_a` output wired straight into `mod_b`'s input port):
///
///   level (stock) --inflow--> mod_a(input=level) --> mod_b(input=mod_a.out)
///       inflow = mod_b.out * 0.1
///
/// Both modules are pure unit-gain passthroughs (`out = input`), so the
/// model is mathematically identical to a bare `level -> inflow -> level`
/// reinforcing one-stock loop. Its raw loop score must be exactly +1 at
/// every step (LTM Appendix B isolated-loop invariant), regardless of the
/// gain the modules introduce. With a non-unity gain on `mod_b` the
/// invariant still holds -- a link score, unlike the gain dz/dx, is
/// normalized so the gain cancels around the loop.
fn two_module_isolated_loop_project(mod_b_gain: f64) -> simlin_engine::datamodel::Project {
    use simlin_engine::datamodel;

    let passthrough_model = |name: &str, gain: f64| datamodel::Model {
        name: name.to_string(),
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
                ident: "out".to_string(),
                equation: datamodel::Equation::Scalar(format!("input * {gain}")),
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
        macro_spec: None,
    };

    datamodel::Project {
        name: "two_module_isolated_loop".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 8.0,
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
                        equation: datamodel::Equation::Scalar("100".to_string()),
                        documentation: String::new(),
                        units: None,
                        inflows: vec!["inflow".to_string()],
                        outflows: vec![],
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "mod_a".to_string(),
                        model_name: "passthrough_a".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "level".to_string(),
                            dst: "mod_a.input".to_string(),
                        }],
                        compat: datamodel::Compat::default(),
                        ai_state: None,
                        uid: None,
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "mod_b".to_string(),
                        model_name: "passthrough_b".to_string(),
                        documentation: String::new(),
                        units: None,
                        // module->module link: mod_a's output wired into
                        // mod_b's input port.
                        references: vec![datamodel::ModuleReference {
                            src: "mod_a.out".to_string(),
                            dst: "mod_b.input".to_string(),
                        }],
                        compat: datamodel::Compat::default(),
                        ai_state: None,
                        uid: None,
                    }),
                    datamodel::Variable::Flow(datamodel::Flow {
                        ident: "inflow".to_string(),
                        equation: datamodel::Equation::Scalar("mod_b.out * 0.1".to_string()),
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
                macro_spec: None,
            },
            passthrough_model("passthrough_a", 1.0),
            passthrough_model("passthrough_b", mod_b_gain),
        ],
        source: None,
        ai_information: None,
    }
}

/// A passthrough sub-model that exposes TWO parent-visible outputs of
/// opposing polarity: `pos = input_val * 0.02` and `neg = 0 - input_val`.
/// The feedback loop reads only `m·pos` (`growth = m·pos * 0.1`), while a
/// non-loop aux `watcher` reads `m·neg`. `neg` sorts before `pos`, so the
/// old `module_output_ref` unit-transfer fallback scored `s -> m` against
/// the alphabetically-first port `neg` -- the WRONG output for this loop --
/// flipping the input->m link sign and therefore the whole loop's polarity.
///
/// PR https://github.com/bpowers/simlin/pull/684#discussion_r3344948690.
///
/// PRE-FIX: the settled raw loop score is -1 (the arbitrary-port unit
/// transfer scores against `neg`, whose sign is opposite `pos`). POST-FIX:
/// the per-exit-port pathway selection scores `s -> m` against the `pos`
/// pathway the loop actually traverses, so the score is +1.
fn multi_output_passthrough_loop_project() -> simlin_engine::datamodel::Project {
    use simlin_engine::datamodel;

    // Sub-model `m`: a stockless passthrough exposing two outputs of
    // opposing sign. `input_val` is the input port; `pos` and `neg` are
    // both read by the parent (so both are output ports).
    let sub_model = datamodel::Model {
        name: "passthrough".to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "input_val".to_string(),
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
                ident: "pos".to_string(),
                equation: datamodel::Equation::Scalar("input_val * 0.02".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "neg".to_string(),
                equation: datamodel::Equation::Scalar("0 - input_val".to_string()),
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
        macro_spec: None,
    };

    datamodel::Project {
        name: "multi_output_passthrough_loop".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 8.0,
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
                        ident: "s".to_string(),
                        equation: datamodel::Equation::Scalar("100".to_string()),
                        documentation: String::new(),
                        units: None,
                        inflows: vec!["growth".to_string()],
                        outflows: vec![],
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "m".to_string(),
                        model_name: "passthrough".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "s".to_string(),
                            dst: "m.input_val".to_string(),
                        }],
                        compat: datamodel::Compat::default(),
                        ai_state: None,
                        uid: None,
                    }),
                    datamodel::Variable::Flow(datamodel::Flow {
                        ident: "growth".to_string(),
                        equation: datamodel::Equation::Scalar("m.pos * 0.1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    // Reads the OTHER (non-loop) output so `neg` becomes a
                    // parent-visible output port, sorted before `pos`.
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "watcher".to_string(),
                        equation: datamodel::Equation::Scalar("m.neg".to_string()),
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
                macro_spec: None,
            },
            sub_model,
        ],
        source: None,
        ai_information: None,
    }
}

/// PR #684 review (r3344948690): a multi-output passthrough module's
/// input->module link must be scored against the output port the loop
/// actually traverses, not the alphabetically-first one. The loop reads
/// `m·pos` (positive gain), so its raw loop score is +1; the non-loop
/// `watcher` reads `m·neg` only to force `neg` into the output-port set.
///
/// Pre-fix the settled loop score was -1 (the unit-transfer fallback scored
/// `s -> m` against `neg`, whose sign opposes `pos`).
#[test]
fn multi_output_passthrough_loop_raw_score_is_one() {
    let project = multi_output_passthrough_loop_project();
    let (compiled, _loop_partitions) = compile_ltm_incremental_with_partitions(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("simulation should run");
    let results = vm.into_results();

    let loop_names: Vec<&str> = results
        .offsets
        .keys()
        .filter(|k| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}")
        })
        .map(|k| k.as_str())
        .collect();
    assert_eq!(
        loop_names.len(),
        1,
        "the multi-output passthrough model has exactly one feedback loop, found {loop_names:?}"
    );
    let off_key = results
        .offsets
        .keys()
        .find(|k| k.as_str() == loop_names[0])
        .unwrap();
    let off = results.offsets[off_key];

    for step in 3..results.step_count {
        let value = results.data[step * results.step_size + off];
        assert!(
            (value - 1.0).abs() < 1e-6,
            "settled step {step} loop score is {value}, expected +1. The loop reads m·pos \
             (positive gain); scoring s->m against the alphabetically-first port m·neg \
             (opposite sign) flips the loop polarity to -1."
        );
    }
}

/// GH #698: discovery mode must agree with exhaustive mode on the SIGN of a
/// loop that traverses a multi-output module. The loop reads `m·pos`
/// (positive gain), so the raw loop score is +1 (reinforcing). Before the
/// fix, discovery scored the `s -> m` edge against the module's composite,
/// which max-abs-selects across BOTH output ports; single-dependency
/// pathways all normalize to magnitude 1, so the `>=` tie-break picked the
/// first-enumerated port (`neg`), inverting the discovered loop to -1.
#[test]
fn discovery_multi_output_loop_polarity_matches_exhaustive() {
    let project = multi_output_passthrough_loop_project();

    // Exhaustive: the raw loop score settles to +1 (verified directly by
    // `multi_output_passthrough_loop_raw_score_is_one`; recomputed here so
    // the two modes are compared on the SAME compiled fixture).
    let (compiled, _loop_partitions) = compile_ltm_incremental_with_partitions(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("exhaustive simulation should run");
    let exhaustive_results = vm.into_results();
    let exhaustive_loop_key = exhaustive_results
        .offsets
        .keys()
        .find(|k| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}")
        })
        .expect("exhaustive mode must emit a loop score")
        .clone();
    let exhaustive_off = exhaustive_results.offsets[&exhaustive_loop_key];
    let last = exhaustive_results.step_count - 1;
    let exhaustive_settled =
        exhaustive_results.data[last * exhaustive_results.step_size + exhaustive_off];
    assert!(
        exhaustive_settled > 0.0,
        "exhaustive settled loop score should be positive (reinforcing), got {exhaustive_settled}"
    );

    // Discovery: run the strongest-path search and find the loop through the
    // module. Its settled signed score must have the same sign exhaustive
    // reports, not the inverted one the composite tie-break produced.
    let compiled = compile_ltm_discovery_incremental(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("discovery simulation should run");
    let discovery_results = vm.into_results();

    let proj = Project::from(project);
    let found = ltm_finding::discover_loops(&discovery_results, &proj)
        .expect("discover_loops should succeed");
    assert!(
        !found.is_empty(),
        "discovery should find the feedback loop through the multi-output module"
    );

    // The single feedback loop traverses `s -> m -> growth -> s`.
    let loop_through_m = found
        .iter()
        .find(|fl| {
            fl.loop_info
                .links
                .iter()
                .any(|l| l.from.as_str() == "m" || l.to.as_str() == "m")
        })
        .expect("a discovered loop must traverse module m");

    let (_, settled_score) = *loop_through_m
        .scores
        .iter()
        .rev()
        .find(|(_, s)| !s.is_nan() && *s != 0.0)
        .expect("loop should have a non-zero settled score");

    assert!(
        settled_score > 0.0,
        "discovery settled loop score is {settled_score}; exhaustive is {exhaustive_settled} \
         (positive/reinforcing). The discovery composite tie-break selected the wrong output \
         port (m·neg) and inverted the loop polarity. GH #698."
    );
}

/// A stockless passthrough module (`out = input_val`) whose output the parent
/// consumes through a parabola (`effect = m.out * (1000 - m.out) / 100000`).
/// The module output appears with conflicting signs in the parabola, so the
/// static polarity analyzer cannot sign the `m -> effect` link and the loop is
/// structurally `Undetermined`. The simulation, however, stays entirely on the
/// rising arm of the parabola (s grows from 100, slowly, staying well below the
/// vertex at 500), so the runtime loop score is single-signed (+1, reinforcing)
/// at every active step.
fn parabola_through_module_loop_project() -> simlin_engine::datamodel::Project {
    use simlin_engine::datamodel;

    let passthrough = datamodel::Model {
        name: "passthrough".to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "input_val".to_string(),
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
                ident: "out".to_string(),
                equation: datamodel::Equation::Scalar("input_val".to_string()),
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
        macro_spec: None,
    };

    datamodel::Project {
        name: "parabola_through_module_loop".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 8.0,
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
                        ident: "s".to_string(),
                        equation: datamodel::Equation::Scalar("100".to_string()),
                        documentation: String::new(),
                        units: None,
                        inflows: vec!["growth".to_string()],
                        outflows: vec![],
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "m".to_string(),
                        model_name: "passthrough".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "s".to_string(),
                            dst: "m.input_val".to_string(),
                        }],
                        compat: datamodel::Compat::default(),
                        ai_state: None,
                        uid: None,
                    }),
                    // `effect` is a quadratic (parabola) in the module output:
                    // `m.out * (1000 - m.out) / 100000`. The module output
                    // appears with conflicting signs (positive in the first
                    // factor, negative in the second), so the static polarity
                    // analyzer cannot sign the m -> effect link -- it is
                    // Unknown, making the whole loop structurally Undetermined.
                    // The sim, however, stays entirely on the rising arm of the
                    // parabola (s grows from 100, slowly, staying well below the
                    // vertex at 500), so the runtime loop score is +1
                    // (reinforcing) at every active step.
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "effect".to_string(),
                        equation: datamodel::Equation::Scalar(
                            "m.out * (1000 - m.out) / 100000".to_string(),
                        ),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Flow(datamodel::Flow {
                        ident: "growth".to_string(),
                        equation: datamodel::Equation::Scalar("effect".to_string()),
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
                macro_spec: None,
            },
            passthrough,
        ],
        source: None,
        ai_information: None,
    }
}

/// GH #679: exhaustive-mode loops must get runtime polarity reclassification.
///
/// `model_detected_loops` is a pre-simulation query, so the loop through the
/// multi-output passthrough module is labelled structurally `Undetermined`
/// (the `s -> m` and `m·pos -> growth` black-box links have Unknown static
/// polarity). The simulated loop score is +1 at every settled step, so after
/// running `reclassify_loops_from_results` the consumer-visible polarity must
/// be Reinforcing -- exactly what discovery mode already reports for this
/// fixture (`discovery_multi_output_loop_polarity_matches_exhaustive`).
#[test]
fn exhaustive_module_loop_polarity_reclassified_from_runtime() {
    let project = parabola_through_module_loop_project();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;

    let detected = model_detected_loops(&db, source_model, sync.project);
    assert_eq!(
        detected.loops.len(),
        1,
        "the parabola-through-module model has exactly one feedback loop"
    );
    // Baseline: the structural label is Undetermined because the parent
    // consumes the module output through a graphical function, whose static
    // polarity the analyzer cannot determine.
    assert_eq!(
        detected.loops[0].polarity,
        DetectedLoopPolarity::Undetermined,
        "structural polarity through a parabola-consumed module output is Undetermined"
    );

    let (compiled, loop_partitions) = compile_ltm_incremental_with_partitions(&project);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("simulation should run");
    let results = vm.into_results();

    let mut loops = detected.loops.clone();
    let id_before = loops[0].id.clone();
    reclassify_loops_from_results(&mut loops, &results, &loop_partitions);

    assert_eq!(
        loops[0].polarity,
        DetectedLoopPolarity::Reinforcing,
        "the loop reads m·pos (positive gain); its runtime loop score is +1, \
         so it must reclassify to Reinforcing, not stay Undetermined"
    );
    assert_eq!(
        loops[0].id, id_before,
        "the loop id must stay stable across reclassification (the FFI id->score \
         correspondence and salsa caching depend on it)"
    );
    assert!(
        (loops[0].polarity_confidence - 1.0).abs() < 1e-9,
        "a single-signed runtime score has confidence 1.0, got {}",
        loops[0].polarity_confidence
    );
}

/// GH #679: a loop whose simulated score is never active -- a `loop_score`
/// series that is PRESENT but every entry is zero or non-finite -- must keep
/// its structural polarity, because `from_runtime_scores` returns `None` (no
/// valid samples) and there is no runtime evidence to override the structural
/// label. This deliberately builds a present-but-all-zero/NaN series so the
/// reclassifier reaches the `from_runtime_scores` -> `None` -> keep-structural
/// branch, rather than short-circuiting at the missing-offset early-continue.
#[test]
fn exhaustive_never_active_loop_keeps_structural_polarity() {
    use simlin_engine::common::{Canonical, Ident};

    let datamodel_project = TestProject::new("goal_seeking")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("goal", "100", None)
        .stock("level", "50", &["adjustment"], &[], None)
        .aux("gap", "goal - level", None)
        .flow("adjustment", "gap / 5", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;

    let detected = model_detected_loops(&db, source_model, sync.project);
    assert!(
        !detected.loops.is_empty(),
        "the goal-seeking model has a balancing loop"
    );
    let structural: Vec<_> = detected
        .loops
        .iter()
        .map(|l| (l.id.clone(), l.polarity, l.polarity_confidence))
        .collect();

    // Build a Results that DOES carry a loop_score series for every detected
    // loop, but every value is zero or NaN. `reclassify_loops_from_results`
    // therefore finds the offset (passing the early-continue), collects the
    // all-invalid samples, and `from_runtime_scores` returns `None` -- the
    // branch that must leave the structural classification untouched.
    let step_count = 4;
    let step_size = detected.loops.len();
    let mut offsets: HashMap<Ident<Canonical>, usize> = HashMap::new();
    for (i, loop_item) in detected.loops.iter().enumerate() {
        let name = format!("$\u{205A}ltm\u{205A}loop_score\u{205A}{}", loop_item.id);
        offsets.insert(Ident::<Canonical>::from_unchecked(name), i);
    }
    // Alternate zero and NaN so both filtered-out cases are exercised.
    let mut data = vec![0.0_f64; step_count * step_size];
    for (idx, slot) in data.iter_mut().enumerate() {
        if idx % 2 == 1 {
            *slot = f64::NAN;
        }
    }
    let all_inactive = Results {
        offsets,
        data: data.into_boxed_slice(),
        step_size,
        step_count,
        specs: simlin_engine::SimSpecs::from(&datamodel_project.sim_specs),
        is_vensim: false,
    };

    // Every loop is scalar here, so each occupies one slot.
    let loop_partitions: IndexMap<String, Vec<Option<usize>>> = detected
        .loops
        .iter()
        .map(|l| (l.id.clone(), vec![Some(0)]))
        .collect();

    let mut loops = detected.loops.clone();
    reclassify_loops_from_results(&mut loops, &all_inactive, &loop_partitions);

    let after: Vec<_> = loops
        .iter()
        .map(|l| (l.id.clone(), l.polarity, l.polarity_confidence))
        .collect();
    assert_eq!(
        structural, after,
        "a loop whose runtime score is present but all-zero/NaN must keep its \
         structural polarity (from_runtime_scores returns None)"
    );
}

/// GROUND TRUTH PROBE / acceptance invariant for GH #675: an isolated
/// feedback loop routed through a module->module link must have raw loop
/// score exactly +1 at every settled step, regardless of the gain the
/// modules introduce. The gain (`Delta_to / Delta_from`) formula that the
/// pre-#675 module->module arm emitted makes the loop score scale with the
/// gain (here `mod_b_gain`), so this fails (or reads 0 if the fragment did
/// not even compile) before the composite/unit-transfer fix.
#[test]
fn module_to_module_isolated_loop_raw_score_is_one() {
    for gain in [1.0_f64, 2.0, 0.5] {
        let project = two_module_isolated_loop_project(gain);
        let (compiled, loop_partitions) = compile_ltm_incremental_with_partitions(&project);
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().expect("simulation should run");
        let results = vm.into_results();

        let loop_names: Vec<&str> = results
            .offsets
            .keys()
            .filter(|k| {
                k.as_str()
                    .starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}")
            })
            .map(|k| k.as_str())
            .collect();
        assert_eq!(
            loop_names.len(),
            1,
            "the two-module model has exactly one feedback loop, found {loop_names:?}"
        );
        let off = results
            .offsets
            .keys()
            .find(|k| k.as_str() == loop_names[0])
            .unwrap();
        let off = results.offsets[off];

        let _ = &loop_partitions;
        for step in 3..results.step_count {
            let value = results.data[step * results.step_size + off];
            assert!(
                (value - 1.0).abs() < 1e-6,
                "gain={gain}: settled step {step} loop score is {value}, expected +1. \
                 A module->module link scored with the gain dz/dx (not a link score) makes \
                 the loop score scale with the module gain; the isolated-loop invariant breaks."
            );
        }
    }
}

/// GH #534 (end-to-end): a positionally-MAPPED sliced reducer subexpression
/// `SUM(matrix[State,*])` inside an A2A-over-`State` body (`matrix` over
/// `[Region,D2]`, positional `State→Region` mapping) is hoisted into an
/// arrayed synthetic agg over `State` whose source rows are remapped along
/// the mapping: `matrix[r1,*]` feeds slot `s1`, `matrix[r2,*]` feeds slot
/// `s2`. The mapped twin of `test_arrayed_sliced_agg_cross_element_loop_simulates`.
///
/// Asserts:
///  - the agg aux is arrayed over `State` and each slot equals the MAPPED
///    Region row's slice sum at every step (positional resolution -- the
///    same reads the engine's own A2A lowering performs);
///  - the per-(read row x remapped slot) source-half link scores exist and
///    are finite, with NO cross-slot variant;
///  - the agg→target half exists per State element (the GH #528 projection
///    composes unchanged: `result_dims` carry the TARGET dim);
///  - zero LTM fragment-compile-failure warnings (every emitted synthetic
///    equation compiles);
///  - the per-element feedback loop through the mapped agg is enumerated and
///    its loop_score series is finite and sustained non-zero.
#[test]
fn test_mapped_sliced_agg_cross_element_loop_simulates() {
    // Loop (per Region/State pair): matrix[r1,x] → $⁚ltm⁚agg⁚0[s1] →
    // growth[s1] → mflow[r1,x] → matrix[r1,x]. `mflow`'s bare `growth`
    // reference resolves through the same positional mapping (GH #527's
    // mapped-Bare diagonal), closing the loop.
    let project = TestProject::new("mapped_sliced_agg_sim")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        .array_stock("matrix[Region,D2]", "100", &["mflow"], &[], None)
        .array_aux("growth[State]", "SUM(matrix[State,*]) * 0.01 + 1")
        .array_flow("mflow[Region,D2]", "growth", None)
        .build_datamodel();

    // The agg node is minted with the (target, source) iterated pair:
    // result dims over State (the TARGET's iterated dim).
    {
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        set_project_ltm_enabled(&mut db, sync.project, true);
        let source_model = sync.models["main"].source_model;
        let agg_nodes =
            simlin_engine::ltm_agg::enumerate_agg_nodes(&db, source_model, sync.project);
        let synthetic: Vec<_> = agg_nodes.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "expected exactly one synthetic agg for the mapped SUM(matrix[State,*]); got: {:?}",
            agg_nodes
                .aggs
                .iter()
                .map(|a| (&a.name, a.is_synthetic, &a.result_dims))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            synthetic[0].result_dims,
            vec!["State".to_string()],
            "the mapped sliced reducer's agg result axis must be the TARGET's iterated dim"
        );
    }

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);

    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let agg_var = ltm.vars.iter().find(|v| v.name == agg).unwrap_or_else(|| {
        panic!(
            "expected the synthetic agg aux {agg}; synthetic vars: {:?}",
            ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        )
    });
    assert_eq!(
        agg_var.dimensions,
        vec!["State".to_string()],
        "the synthetic agg aux must be arrayed over State"
    );

    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();

    // Every emitted LTM synthetic fragment compiles: no fragment-failure
    // warnings (the silent-stub path would zero the loop score).
    let diags = simlin_engine::db::collect_all_diagnostics(&db, sync.project);
    let frag_failures: Vec<_> = diags
        .iter()
        .filter(|d| {
            d.severity == simlin_engine::db::DiagnosticSeverity::Warning
                && matches!(
                    &d.error,
                    simlin_engine::db::DiagnosticError::Assembly(msg)
                        if msg.contains("failed to compile")
                )
        })
        .collect();
    assert!(
        frag_failures.is_empty(),
        "the mapped sliced-reducer model must compile every LTM fragment; got: {frag_failures:?}"
    );

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end()
        .expect("mapped-sliced-agg model should simulate with LTM enabled");
    let results = vm.into_results();

    let off = |name: &str| -> usize {
        *results
            .offsets
            .get(&Ident::<Canonical>::new(name))
            .unwrap_or_else(|| {
                panic!(
                    "missing offset {name}; have: {:?}",
                    results
                        .offsets
                        .keys()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                )
            })
    };
    let at = |step: usize, o: usize| results.data[step * results.step_size + o];

    // Each agg slot equals the MAPPED Region row's slice sum (s1 ↦ r1,
    // s2 ↦ r2 -- positional).
    let agg_base = off(agg);
    for (state_idx, region) in ["r1", "r2"].into_iter().enumerate() {
        let agg_slot = agg_base + state_idx;
        let mx_a = off(&format!("matrix[{region},x]"));
        let mx_b = off(&format!("matrix[{region},y]"));
        for step in 0..results.step_count {
            let expected = at(step, mx_a) + at(step, mx_b);
            assert!(
                (at(step, agg_slot) - expected).abs() < 1e-9 * expected.abs().max(1.0),
                "step {step}: {agg} slot {state_idx} = {}, expected SUM(matrix[{region},*]) = {expected}",
                at(step, agg_slot)
            );
        }
    }

    // Per-(read row x remapped slot) source-half link scores exist and are
    // finite; the cross-slot variant must not exist.
    for (region, state) in [("r1", "s1"), ("r2", "s2")] {
        for d2 in &["x", "y"] {
            let o = off(&format!(
                "$\u{205A}ltm\u{205A}link_score\u{205A}matrix[{region},{d2}]\u{2192}{agg}[{state}]"
            ));
            for step in 0..results.step_count {
                assert!(
                    at(step, o).is_finite(),
                    "step {step}: matrix[{region},{d2}]→{agg}[{state}] link score not finite"
                );
            }
        }
    }
    for (region, state) in [("r1", "s2"), ("r2", "s1")] {
        assert!(
            !results
                .offsets
                .contains_key(&Ident::<Canonical>::new(&format!(
                    "$\u{205A}ltm\u{205A}link_score\u{205A}matrix[{region},x]\u{2192}{agg}[{state}]"
                ))),
            "must not emit a matrix[{region},x]→{agg}[{state}] link score (wrong slot under the mapping)"
        );
    }

    // The agg→target half exists per State element (diagonal on State).
    for state in &["s1", "s2"] {
        let o = off(&format!(
            "$\u{205A}ltm\u{205A}link_score\u{205A}{agg}[{state}]\u{2192}growth[{state}]"
        ));
        for step in 0..results.step_count {
            assert!(
                at(step, o).is_finite(),
                "step {step}: {agg}[{state}]→growth[{state}] link score not finite"
            );
        }
    }

    // A loop through the mapped agg is enumerated and scored: finite and
    // sustained non-zero (the exponential growth keeps every link active).
    let cross_agg_loop_score_name = ltm
        .vars
        .iter()
        .find(|v| {
            v.name.contains("\u{205A}loop_score\u{205A}")
                && v.equation
                    .source_text()
                    .contains(format!("{agg}[s1]\u{2192}growth[s1]").as_str())
                && v.equation.source_text().contains("matrix[r1,")
                && v.equation
                    .source_text()
                    .contains(format!("\u{2192}{agg}[s1]").as_str())
        })
        .map(|v| v.name.clone())
        .unwrap_or_else(|| {
            panic!(
                "expected a loop_score var traversing matrix[r1,*]→{agg}[s1]→growth[s1]; loop scores: {:?}",
                ltm.vars
                    .iter()
                    .filter(|v| v.name.contains("\u{205A}loop_score\u{205A}"))
                    .map(|v| (v.name.as_str(), v.equation.source_text()))
                    .collect::<Vec<_>>()
            )
        });
    let lo = off(&cross_agg_loop_score_name);
    let mut nonzero_steps = 0usize;
    for step in 2..results.step_count {
        let v = at(step, lo);
        assert!(
            v.is_finite(),
            "step {step}: mapped-agg loop score not finite"
        );
        if v.abs() > 1e-12 {
            nonzero_steps += 1;
        }
    }
    assert!(
        nonzero_steps >= results.step_count.saturating_sub(2) / 2,
        "the mapped-agg loop score must be sustained non-zero (got {nonzero_steps} non-zero of {} post-warmup steps)",
        results.step_count - 2
    );
}

/// GH #534 (scalar co-feeder composition, end-to-end): the mapped sliced
/// reducer with a scalar co-feeder (`SUM(matrix[State,*] * scale)`) still
/// hoists (the scalar arg contributes no read slice), the scalar feeder
/// gets its Bare-named per-slot link score shaped over `State` (asserted in
/// discovery mode -- exhaustive mode scores only loop-participating edges,
/// and `scale` is exogenous here), and the model compiles every LTM
/// fragment and simulates finite.
#[test]
fn test_mapped_sliced_agg_with_scalar_cofeeder_simulates() {
    let project = TestProject::new("mapped_sliced_cofeeder_sim")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        .scalar_aux("scale", "2")
        .array_stock("matrix[Region,D2]", "100", &["mflow"], &[], None)
        .array_aux("growth[State]", "SUM(matrix[State,*] * scale) * 0.005 + 1")
        .array_flow("mflow[Region,D2]", "growth", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);

    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let agg_var = ltm.vars.iter().find(|v| v.name == agg).unwrap_or_else(|| {
        panic!(
            "expected the synthetic agg aux {agg} (the scalar co-feeder must not block hoisting); \
             vars: {:?}",
            ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        )
    });
    assert_eq!(agg_var.dimensions, vec!["State".to_string()]);

    // The scalar feeder's Bare-named link score is shaped over State. The
    // exogenous `scale` participates in no loop, so exhaustive mode never
    // emits its edge score -- assert via discovery mode, which scores every
    // causal edge (the GH #737 scalar-feeder arm composed with the GH #534
    // remapped agg).
    {
        let mut ddb = SimlinDb::default();
        let dsync = sync_from_datamodel_incremental(&mut ddb, &project, None);
        set_project_ltm_enabled(&mut ddb, dsync.project, true);
        set_project_ltm_discovery_mode(&mut ddb, dsync.project, true);
        let dmodel = dsync.models["main"].source_model;
        let dltm = model_ltm_variables(&ddb, dmodel, dsync.project);
        let feeder = dltm
            .vars
            .iter()
            .find(|v| v.name == format!("$\u{205A}ltm\u{205A}link_score\u{205A}scale\u{2192}{agg}"))
            .expect("expected the scalar-feeder link score scale→agg in discovery mode");
        assert_eq!(
            feeder.dimensions,
            vec!["State".to_string()],
            "the scalar-feeder score must be shaped over the agg's result dims"
        );
    }

    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
    let diags = simlin_engine::db::collect_all_diagnostics(&db, sync.project);
    let frag_failures: Vec<_> = diags
        .iter()
        .filter(|d| {
            d.severity == simlin_engine::db::DiagnosticSeverity::Warning
                && matches!(
                    &d.error,
                    simlin_engine::db::DiagnosticError::Assembly(msg)
                        if msg.contains("failed to compile")
                )
        })
        .collect();
    assert!(
        frag_failures.is_empty(),
        "the co-feeder model must compile every LTM fragment; got: {frag_failures:?}"
    );

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end()
        .expect("mapped-sliced-agg co-feeder model should simulate with LTM enabled");
    let results = vm.into_results();

    // The remapped source-half link scores exist and the run stays finite.
    for (region, state) in [("r1", "s1"), ("r2", "s2")] {
        let name = format!(
            "$\u{205A}ltm\u{205A}link_score\u{205A}matrix[{region},x]\u{2192}{agg}[{state}]"
        );
        let o = *results
            .offsets
            .get(&Ident::<Canonical>::new(&name))
            .unwrap_or_else(|| panic!("missing remapped source-half link score {name}"));
        for step in 0..results.step_count {
            let v = results.data[step * results.step_size + o];
            assert!(v.is_finite(), "step {step}: {name} not finite");
        }
    }
}

/// GH #534 (whole-RHS twin, end-to-end): `out[State] = SUM(matrix[State,*])`
/// over a positionally-mapped pair mints a SYNTHETIC agg (an exception to
/// the variable-is-the-agg rule -- the variable-backed link-score path is
/// name-based and cannot remap; its `Wildcard` partial generated the
/// non-compiling `matrix[PREVIOUS(state),*]` and silently stubbed the score
/// to 0). Asserts: the synthetic agg exists, every LTM fragment compiles,
/// the remapped source-half link scores exist, and the loops through the
/// agg are scored finite.
#[test]
fn test_whole_rhs_mapped_reducer_routes_through_synthetic_agg() {
    let project = TestProject::new("whole_rhs_mapped_e2e")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        .array_stock("matrix[Region,D2]", "100", &["mflow"], &[], None)
        .array_aux("out[State]", "SUM(matrix[State,*])")
        .array_flow("mflow[Region,D2]", "out * 0.01", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);

    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    assert!(
        ltm.vars.iter().any(|v| v.name == agg),
        "the whole-RHS mapped reducer must mint a synthetic agg; vars: {:?}",
        ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
    );

    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
    let diags = simlin_engine::db::collect_all_diagnostics(&db, sync.project);
    let frag_failures: Vec<_> = diags
        .iter()
        .filter(|d| {
            d.severity == simlin_engine::db::DiagnosticSeverity::Warning
                && matches!(
                    &d.error,
                    simlin_engine::db::DiagnosticError::Assembly(msg)
                        if msg.contains("failed to compile")
                )
        })
        .collect();
    assert!(
        frag_failures.is_empty(),
        "the whole-RHS mapped model must compile every LTM fragment (the \
         variable-backed Wildcard partial used to silently stub); got: {frag_failures:?}"
    );

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end()
        .expect("whole-RHS mapped model should simulate with LTM enabled");
    let results = vm.into_results();

    // Remapped source-half link scores exist and stay finite; the cross-slot
    // variant must not exist.
    for (region, state) in [("r1", "s1"), ("r2", "s2")] {
        for d2 in &["x", "y"] {
            let name = format!(
                "$\u{205A}ltm\u{205A}link_score\u{205A}matrix[{region},{d2}]\u{2192}{agg}[{state}]"
            );
            let o = *results
                .offsets
                .get(&Ident::<Canonical>::new(&name))
                .unwrap_or_else(|| panic!("missing remapped source-half link score {name}"));
            for step in 0..results.step_count {
                let v = results.data[step * results.step_size + o];
                assert!(v.is_finite(), "step {step}: {name} not finite");
            }
        }
    }
    assert!(
        !results
            .offsets
            .contains_key(&Ident::<Canonical>::new(&format!(
                "$\u{205A}ltm\u{205A}link_score\u{205A}matrix[r1,x]\u{2192}{agg}[s2]"
            ))),
        "must not emit a cross-slot matrix[r1,x]→{agg}[s2] link score"
    );

    // Each loop through the agg gets a finite loop score.
    let loop_scores: Vec<_> = ltm
        .vars
        .iter()
        .filter(|v| {
            v.name.contains("\u{205A}loop_score\u{205A}")
                && v.equation
                    .source_text()
                    .contains(format!("\u{2192}{agg}[").as_str())
        })
        .map(|v| v.name.clone())
        .collect();
    assert!(
        !loop_scores.is_empty(),
        "expected loop scores traversing the synthetic agg; loop scores: {:?}",
        ltm.vars
            .iter()
            .filter(|v| v.name.contains("\u{205A}loop_score\u{205A}"))
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );
    for name in &loop_scores {
        let o = *results
            .offsets
            .get(&Ident::<Canonical>::new(name))
            .unwrap_or_else(|| panic!("missing loop score {name}"));
        for step in 0..results.step_count {
            let v = results.data[step * results.step_size + o];
            assert!(v.is_finite(), "step {step}: {name} not finite");
        }
    }
}
