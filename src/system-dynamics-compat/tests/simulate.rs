// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use std::iter::FromIterator;
use std::rc::Rc;

use csv;
use float_cmp::approx_eq;

use system_dynamics_compat::xmile;
use system_dynamics_engine::{canonicalize, Method, Project, Results, SimSpecs, Simulation};

const OUTPUT_FILES: &[(&str, u8)] = &[("output.csv", ',' as u8), ("output.tab", '\t' as u8)];

// these columns are either Vendor specific or otherwise not important.
const IGNORABLE_COLS: &[&str] = &["saveper", "initial_time", "final_time", "time_step"];

static TEST_MODELS: &[&str] = &[
    "test/test-models/tests/delays2/delays.xmile",
    "test/test-models/tests/smooth_and_stock/test_smooth_and_stock.xmile",
    "test/test-models/samples/SIR/SIR.xmile",
    "test/test-models/samples/SIR/SIR_reciprocal-dt.xmile",
    "test/test-models/samples/teacup/teacup.xmile",
    "test/test-models/samples/teacup/teacup_w_diagram.xmile",
    "test/test-models/tests/trig/test_trig.xmile",
    "test/test-models/tests/lookups_inline/test_lookups_inline.xmile",
    "test/test-models/tests/comparisons/comparisons.xmile",
    "test/test-models/tests/sqrt/test_sqrt.xmile",
    "test/test-models/tests/abs/test_abs.xmile",
    "test/test-models/tests/constant_expressions/test_constant_expressions.xmile",
    "test/test-models/tests/lookups/test_lookups.xmile",
    "test/test-models/tests/lookups/test_lookups_no-indirect.xmile",
    "test/test-models/tests/line_breaks/test_line_breaks.xmile",
    "test/test-models/tests/parentheses/test_parens.xmile",
    "test/test-models/tests/builtin_max/builtin_max.xmile",
    "test/test-models/tests/number_handling/test_number_handling.xmile",
    "test/test-models/tests/if_stmt/if_stmt.xmile",
    "test/test-models/tests/game/test_game.xmile",
    "test/test-models/tests/eval_order/eval_order.xmile",
    "test/test-models/tests/xidz_zidz/xidz_zidz.xmile",
    "test/test-models/tests/exponentiation/exponentiation.xmile",
    "test/test-models/tests/logicals/test_logicals.xmile",
    "test/test-models/tests/limits/test_limits.xmile",
    "test/test-models/tests/line_continuation/test_line_continuation.xmile",
    "test/test-models/tests/ln/test_ln.xmile",
    "test/test-models/tests/model_doc/model_doc.xmile",
    "test/test-models/tests/reference_capitalization/test_reference_capitalization.xmile",
    "test/test-models/tests/log/test_log.xmile",
    "test/test-models/tests/function_capitalization/test_function_capitalization.xmile",
    "test/test-models/tests/chained_initialization/test_chained_initialization.xmile",
    "test/test-models/tests/exp/test_exp.xmile",
    "test/test-models/tests/builtin_min/builtin_min.xmile",
    "test/test-models/samples/bpowers-hares_and_lynxes_modules/model.xmile",
];

fn load_csv(file_path: &str, delimiter: u8) -> Result<Results, Box<dyn Error>> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .from_path(file_path)?;

    let header = rdr.headers().unwrap();
    let offsets: HashMap<String, usize> =
        HashMap::from_iter(header.iter().enumerate().map(|(i, r)| (canonicalize(r), i)));

    let step_size = offsets.len();
    let mut step_data: Vec<Vec<f64>> = Vec::new();
    let mut step_count = 0;

    for result in rdr.records() {
        let record = result?;

        let mut row = vec![0.0; step_size];
        for (i, field) in record.iter().enumerate() {
            use std::str::FromStr;
            row[i] = match f64::from_str(field.trim()) {
                Ok(n) => n,
                Err(err) => {
                    eprintln!("invalid: '{}': {}", field.trim(), err);
                    assert!(false);
                    0.0
                }
            };
        }

        step_data.push(row);
        step_count += 1;
    }

    let step_data: Vec<f64> = step_data.into_iter().flatten().collect();

    Ok(Results {
        offsets,
        data: step_data.into_boxed_slice(),
        step_size,
        step_count,
        specs: SimSpecs {
            start: 0.0,
            stop: 0.0,
            dt: 0.0,
            save_step: 0.0,
            method: Method::Euler,
        },
    })
}

fn load_expected_results(xmile_path: &str) -> Results {
    let xmile_name = std::path::Path::new(xmile_path).file_name().unwrap();
    let dir_path = &xmile_path[0..(xmile_path.len() - xmile_name.len())];
    let dir_path = std::path::Path::new(dir_path);

    for (output_file, delimiter) in OUTPUT_FILES.iter() {
        let output_path = dir_path.join(output_file);
        if !output_path.exists() {
            continue;
        }
        return load_csv(&output_path.to_string_lossy(), *delimiter).unwrap();
    }

    unreachable!();
}

fn simulate_path(xmile_path: &str) {
    eprintln!("model: {}", xmile_path);
    let f = File::open(xmile_path).unwrap();
    let mut f = BufReader::new(f);

    let datamodel_project = xmile::project_from_reader(&mut f);
    if let Err(ref err) = datamodel_project {
        eprintln!("model '{}' error: {}", xmile_path, err);
    }
    let project = Project::from(datamodel_project.unwrap());

    let project = Rc::new(project);
    let sim = Simulation::new(&project, "main").unwrap();
    let results = sim.run_to_end();
    assert!(results.is_ok());

    let results = results.unwrap();
    let expected = load_expected_results(xmile_path);

    assert_eq!(expected.step_count, results.step_count);

    let mut step = 0;
    for (expected_row, results_row) in expected.iter().zip(results.iter()) {
        for ident in expected.offsets.keys() {
            let expected = expected_row[expected.offsets[ident]];
            if !results.offsets.contains_key(ident) && IGNORABLE_COLS.contains(&ident.as_str()) {
                continue;
            }
            if !results.offsets.contains_key(ident) {
                panic!("output missing variable '{}'", ident);
            }
            let off = results.offsets[ident];
            let actual = results_row[off];

            let around_zero = approx_eq!(f64, expected, 0.0, epsilon = 3e-6)
                && approx_eq!(f64, actual, 0.0, epsilon = 1e-6);

            // this ulps determined empirically /shrug
            if !around_zero && !approx_eq!(f64, expected, actual, ulps = 300000000000) {
                eprintln!(
                    "step {}: {}: {} (expected) != {} (actual)",
                    step, ident, expected, actual
                );
                assert!(false);
            }
        }

        step += 1;
    }

    assert_eq!(expected.step_count, step);
}

#[test]
fn simulates_models_correctly() {
    for &path in TEST_MODELS {
        let file_path = format!("../../{}", path);
        simulate_path(file_path.as_str());
    }
}

#[test]
fn simulates_arrays() {
    // simulate_path("../../test/arrays.stmx");
}

#[test]
fn bad_model_name() {
    let f = File::open(format!("../../{}", TEST_MODELS[0])).unwrap();
    let mut f = BufReader::new(f);
    let datamodel_project = xmile::project_from_reader(&mut f).unwrap();
    let project = Project::from(datamodel_project);
    let project = Rc::new(project);
    assert!(Simulation::new(&project, "blerg").is_err());
}
