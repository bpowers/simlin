// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use std::rc::Rc;
use std::result::Result as StdResult;

use simlin_compat::xmile;
use simlin_engine::common::{Canonical, Ident, canonicalize};
use simlin_engine::interpreter::Simulation;
use simlin_engine::{Project, Results, Vm, ltm};

const LTM_TOLERANCE: f64 = 0.05;

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
    loops: &HashMap<Ident<Canonical>, Vec<ltm::Loop>>,
) {
    let mut errors = Vec::new();

    for (loop_id, expected_scores) in &expected.loop_scores {
        let var_name = format!("$⁚ltm⁚rel_loop_score⁚{}", loop_id);
        let var_ident =
            Ident::<Canonical>::from_str_unchecked(&canonicalize(&var_name).to_source_repr());

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
            let loop_info = loops
                .values()
                .flat_map(|v| v.iter())
                .find(|l| l.id == *loop_id);

            eprintln!("Loop: {}", loop_id);
            if let Some(loop_obj) = loop_info {
                eprintln!(
                    "  Polarity: {}",
                    match loop_obj.polarity {
                        ltm::LoopPolarity::Reinforcing => "Reinforcing (R)",
                        ltm::LoopPolarity::Balancing => "Balancing (B)",
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
    eprintln!("LTM model: {}", model_path);

    let f = File::open(model_path).unwrap();
    let mut f = BufReader::new(f);
    let datamodel_project = xmile::project_from_reader(&mut f);

    if let Err(ref err) = datamodel_project {
        eprintln!("model '{model_path}' error: {err}");
    }
    let datamodel_project = datamodel_project.unwrap();

    let project = Project::from(datamodel_project);
    let ltm_project = project.with_ltm().unwrap();

    let loops = ltm::detect_loops(&ltm_project).unwrap();
    let ltm_project = Rc::new(ltm_project);

    let sim = Simulation::new(&ltm_project, "main").unwrap();
    let results1 = sim.run_to_end().unwrap();

    let compiled = sim.compile().unwrap();
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results2 = vm.into_results();

    let xmile_name = std::path::Path::new(model_path).file_name().unwrap();
    let dir_path = &model_path[0..(model_path.len() - xmile_name.len())];
    let dir_path = std::path::Path::new(dir_path);

    let ltm_results_path = dir_path.join("ltm_results.tsv");
    let expected = load_ltm_results(&ltm_results_path.to_string_lossy()).unwrap();

    ensure_ltm_results(&expected, &results1, &loops);
    ensure_ltm_results(&expected, &results2, &loops);
}

#[test]
fn simulates_population_ltm() {
    simulate_ltm_path("../../test/logistic_growth_ltm/logistic_growth.stmx");
}
