// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::rc::Rc;

use float_cmp::approx_eq;

#[cfg(feature = "file_io")]
use simlin_engine::FilesystemDataProvider;
use simlin_engine::common::{Canonical, Ident};
use simlin_engine::interpreter::Simulation;
use simlin_engine::serde::{deserialize, serialize};
use simlin_engine::{Project, Results, Vm, project_io};
use simlin_engine::{load_csv, load_dat, open_vensim, open_vensim_with_data, xmile};

const OUTPUT_FILES: &[(&str, u8)] = &[("output.csv", b','), ("output.tab", b'\t')];

// these columns are either Vendor specific or otherwise not important.
const IGNORABLE_COLS: &[&str] = &["saveper", "initial_time", "final_time", "time_step"];

/// Check if a variable name is a Vensim-specific internal delay/smooth variable
/// These have formats like "#d8>DELAY3#[A1]" or "#d8>DELAY3>RT2#[A1]"
fn is_vensim_internal_module_var(name: &str) -> bool {
    // Vensim internal variables start with # and contain >
    name.starts_with('#') && name.contains('>')
}

static TEST_MODELS: &[&str] = &[
    // failing testcases (various reasons)
    // "test/test-models/tests/arguments/test_arguments.xmile",
    // "test/test-models/tests/delay_parentheses/test_delay_parentheses.xmile",
    // "test/test-models/tests/delay_pipeline/test_pipeline_delays.xmile",
    // "test/test-models/tests/macro_expression/test_macro_expression.xmile",
    // "test/test-models/tests/macro_multi_expression/test_macro_multi_expression.xmile",
    // "test/test-models/tests/macro_stock/test_macro_stock.xmile",
    // "test/test-models/tests/rounding/test_rounding.xmile",
    // "test/test-models/tests/special_characters/test_special_variable_names.xmile",
    // "test/test-models/tests/stocks_with_expressions/test_stock_with_expression.xmile",

    // failing testcase: xmutil doesn't handle this correctly
    // "test/test-models/tests/subscript_mixed_assembly/test_subscript_mixed_assembly.xmile",
    //
    "test/test-models/samples/arrays/a2a/a2a.stmx",
    "test/test-models/samples/arrays/non-a2a/non-a2a.stmx",
    "test/test-models/samples/bpowers-hares_and_lynxes_modules/model.xmile",
    "test/test-models/samples/SIR/SIR.xmile",
    "test/test-models/samples/SIR/SIR_reciprocal-dt.xmile",
    "test/test-models/samples/teacup/teacup_w_diagram.xmile",
    "test/test-models/samples/teacup/teacup.xmile",
    "test/test-models/tests/abs/test_abs.xmile",
    "test/test-models/tests/builtin_max/builtin_max.xmile",
    "test/test-models/tests/builtin_mean/builtin_mean.xmile",
    "test/test-models/tests/builtin_min/builtin_min.xmile",
    "test/test-models/tests/builtin_int/builtin_int.xmile",
    "test/test-models/tests/chained_initialization/test_chained_initialization.xmile",
    "test/test-models/tests/comparisons/comparisons.xmile",
    "test/test-models/tests/constant_expressions/test_constant_expressions.xmile",
    "test/test-models/tests/delays2/delays.xmile",
    "test/test-models/tests/euler_step_vs_saveper/test_euler_step_vs_saveper.xmile",
    "test/test-models/tests/eval_order/eval_order.xmile",
    "test/test-models/tests/exponentiation/exponentiation.xmile",
    "test/test-models/tests/exp/test_exp.xmile",
    "test/test-models/tests/function_capitalization/test_function_capitalization.xmile",
    "test/test-models/tests/game/test_game.xmile",
    "test/test-models/tests/if_stmt/if_stmt.xmile",
    "test/test-models/tests/input_functions/test_inputs.xmile",
    "test/test-models/tests/limits/test_limits.xmile",
    "test/test-models/tests/line_breaks/test_line_breaks.xmile",
    "test/test-models/tests/line_continuation/test_line_continuation.xmile",
    "test/test-models/tests/ln/test_ln.xmile",
    "test/test-models/tests/logicals/test_logicals.xmile",
    "test/test-models/tests/log/test_log.xmile",
    "test/test-models/tests/lookups_inline_bounded/test_lookups_inline_bounded.xmile",
    "test/test-models/tests/lookups_inline/test_lookups_inline.xmile",
    "test/test-models/tests/lookups/test_lookups_no-indirect.xmile",
    // "test/test-models/tests/lookups/test_lookups.xmile",
    "test/test-models/tests/lookups_simlin/test_lookups.xmile",
    "test/test-models/tests/lookups_with_expr/test_lookups_with_expr.xmile",
    "test/test-models/tests/model_doc/model_doc.xmile",
    "test/test-models/tests/number_handling/test_number_handling.xmile",
    "test/test-models/tests/parentheses/test_parens.xmile",
    "test/test-models/tests/reference_capitalization/test_reference_capitalization.xmile",
    "test/test-models/tests/smooth_and_stock/test_smooth_and_stock.xmile",
    "test/test-models/tests/sqrt/test_sqrt.xmile",
    "test/test-models/tests/stocks_with_expressions/test_stock_with_expression.xmile",
    "test/test-models/tests/subscript_1d_arrays/test_subscript_1d_arrays.xmile",
    "test/test-models/tests/subscript_2d_arrays/test_subscript_2d_arrays.xmile",
    "test/test-models/tests/subscript_3d_arrays/test_subscript_3d_arrays.xmile",
    "test/test-models/tests/subscript_docs/subscript_docs.xmile",
    "test/test-models/tests/subscript_individually_defined_1_of_2d_arrays_from_floats/subscript_individually_defined_1_of_2d_arrays_from_floats.xmile",
    "test/test-models/tests/subscript_individually_defined_1_of_2d_arrays/subscript_individually_defined_1_of_2d_arrays.xmile",
    "test/test-models/tests/subscript_multiples/test_multiple_subscripts.xmile",
    "test/test-models/tests/subscript_selection/subscript_selection.xmile",
    "test/test-models/tests/trend/test_trend.xmile",
    "test/test-models/tests/trig/test_trig.xmile",
    "test/test-models/tests/xidz_zidz/xidz_zidz.xmile",
    "test/test-models/tests/unicode_characters/unicode_test_model.xmile",
];

fn load_expected_results(xmile_path: &str) -> Option<Results> {
    let xmile_name = std::path::Path::new(xmile_path).file_name().unwrap();
    let dir_path = &xmile_path[0..(xmile_path.len() - xmile_name.len())];
    let dir_path = std::path::Path::new(dir_path);

    for (output_file, delimiter) in OUTPUT_FILES.iter() {
        let output_path = dir_path.join(output_file);
        if !output_path.exists() {
            continue;
        }
        return Some(load_csv(&output_path.to_string_lossy(), *delimiter).unwrap());
    }

    let dat_file = xmile_path.replace(".xmile", ".dat");
    let dat_path = std::path::Path::new(&dat_file);
    if dat_path.exists() {
        return Some(load_dat(&dat_file).unwrap());
    }

    None
}

/// Compare VDF reference data against simulation results with cross-simulator
/// tolerance. VDF stores f32 (~7 digits) and Vensim's integration may differ
/// from ours, so we allow up to 1% relative error.
/// Variables present in `results` but not in `vdf_expected` are skipped
/// (they may be internal module variables without VDF entries).
fn ensure_vdf_results(vdf_expected: &Results, results: &Results) {
    assert_eq!(vdf_expected.step_count, results.step_count);

    let mut matched = 0;
    let mut max_rel_error: f64 = 0.0;
    let mut max_rel_ident = String::new();
    let mut failures = 0;
    let step_count = vdf_expected.step_count;

    for ident in vdf_expected.offsets.keys() {
        if !results.offsets.contains_key(ident) {
            continue;
        }
        let vdf_off = vdf_expected.offsets[ident];
        let sim_off = results.offsets[ident];
        matched += 1;

        for step in 0..step_count {
            let expected = vdf_expected.data[step * vdf_expected.step_size + vdf_off];
            let actual = results.data[step * results.step_size + sim_off];

            if expected.is_nan() || actual.is_nan() {
                continue;
            }

            let max_val = expected.abs().max(actual.abs()).max(1e-10);
            let rel_err = (expected - actual).abs() / max_val;
            if rel_err > max_rel_error {
                max_rel_error = rel_err;
                max_rel_ident = format!("{ident} (step {step})");
            }

            // 1% relative tolerance for cross-simulator comparison
            if rel_err > 0.01 {
                failures += 1;
                if failures <= 5 {
                    eprintln!(
                        "FAIL step {step}: {ident}: {expected} (vdf) != {actual} (sim), rel_err={rel_err:.6}"
                    );
                }
            }
        }
    }

    eprintln!("VDF comparison: {matched} variables matched across {step_count} time steps");
    eprintln!("  Max relative error: {max_rel_error:.6} at {max_rel_ident}");
    if failures > 0 {
        eprintln!("  {failures} comparisons exceeded 1% tolerance");
        panic!("VDF comparison failed with {failures} tolerance violations");
    }
}

fn ensure_results(expected: &Results, results: &Results) {
    assert_eq!(expected.step_count, results.step_count);
    assert_eq!(expected.iter().len(), results.iter().len());

    let expected_results = expected;

    let mut step = 0;
    for (expected_row, results_row) in expected.iter().zip(results.iter()) {
        for ident in expected.offsets.keys() {
            let expected = expected_row[expected.offsets[ident]];
            if !results.offsets.contains_key(ident)
                && (IGNORABLE_COLS.contains(&ident.as_str())
                    || is_vensim_internal_module_var(ident.as_str()))
            {
                continue;
            }
            if !results.offsets.contains_key(ident) {
                panic!("output missing variable '{ident}'");
            }
            let off = results.offsets[ident];
            let actual = results_row[off];

            let around_zero = approx_eq!(f64, expected, 0.0, epsilon = 3e-6)
                && approx_eq!(f64, actual, 0.0, epsilon = 1e-6);

            if !around_zero {
                let (exp_cmp, act_cmp, epsilon) = if results.is_vensim || expected_results.is_vensim
                {
                    // Vensim outputs ~6 significant figures. Use relative comparison
                    // to handle large magnitudes (where small relative errors become
                    // large absolute errors). For small values, maintain the original
                    // absolute tolerance of 2e-3 so we don't become too strict.
                    let max_val = expected.abs().max(actual.abs()).max(1e-10);
                    let relative_eps = max_val * 5e-6;
                    (expected, actual, relative_eps.max(2e-3))
                } else {
                    (expected, actual, 2e-3)
                };

                if !approx_eq!(f64, exp_cmp, act_cmp, epsilon = epsilon) {
                    eprintln!("step {step}: {ident}: {expected} (expected) != {actual} (actual)");
                    panic!("not equal");
                }
            }
        }

        step += 1;
    }

    assert_eq!(expected.step_count, step);

    // UNKNOWN is a sentinal value we use -- it should never show up
    // unless we've wrongly sized our data slices
    assert!(
        !results
            .offsets
            .contains_key(&Ident::<Canonical>::from_str_unchecked("UNKNOWN"))
    );
}

fn simulate_path(xmile_path: &str) {
    eprintln!("model: {xmile_path}");

    // first read-in the XMILE model, convert it to our own representation,
    // and simulate it using our tree-walking interpreter
    let (datamodel_project, sim, results1) = {
        let f = File::open(xmile_path).unwrap();
        let mut f = BufReader::new(f);

        let datamodel_project = xmile::project_from_reader(&mut f);
        if let Err(ref err) = datamodel_project {
            eprintln!("model '{xmile_path}' error: {err}");
        }
        let datamodel_project = datamodel_project.unwrap();
        let project = Rc::new(Project::from(datamodel_project.clone()));
        let sim = Simulation::new(&project, "main").unwrap();

        // sim.debug_print_runlists("main");
        let results = sim.run_to_end();
        assert!(results.is_ok());
        (datamodel_project, sim, results.unwrap())
    };

    // next simulate the model using our bytecode VM
    let results2 = {
        let compiled = sim.compile();

        if let Err(ref e) = compiled {
            eprintln!("Compilation error: {:?}", e);
        }
        assert!(compiled.is_ok(), "compile failed: {:?}", compiled);
        let compiled_sim = compiled.unwrap();

        let mut vm = Vm::new(compiled_sim).unwrap();
        // vm.debug_print_bytecode("main");
        vm.run_to_end().unwrap();
        vm.into_results()
    };

    // ensure the two results match each other
    ensure_results(&results1, &results2);

    // also ensure they match our reference results
    let expected = load_expected_results(xmile_path).unwrap();
    ensure_results(&expected, &results1);
    ensure_results(&expected, &results2);

    // serialized our project through protobufs and ensure we don't see problems
    let results3 = {
        use simlin_engine::prost::Message;

        let pb_project_inner = serialize(&datamodel_project).unwrap();
        let pb_project = &pb_project_inner;
        let mut buf = Vec::with_capacity(pb_project.encoded_len());
        pb_project.encode(&mut buf).unwrap();

        let datamodel_project2 = deserialize(project_io::Project::decode(&*buf).unwrap());
        assert_eq!(datamodel_project, datamodel_project2);
        let project = Project::from(datamodel_project2);
        let project = Rc::new(project);
        let sim = Simulation::new(&project, "main").unwrap();
        let compiled_sim = sim.compile().unwrap();
        let mut vm = Vm::new(compiled_sim).unwrap();
        vm.run_to_end().unwrap();
        vm.into_results()
    };
    ensure_results(&expected, &results3);

    // serialize our project back to XMILE
    let serialized_xmile = xmile::project_to_xmile(&datamodel_project).unwrap();

    // and then read it back in from the XMILE string and simulate it
    let (roundtripped_project, results4) = {
        let mut xmile_reader = BufReader::new(serialized_xmile.as_bytes());
        // eprintln!("xmile:\n{}", serialized_xmile);
        let roundtripped_project = xmile::project_from_reader(&mut xmile_reader).unwrap();

        let project = Project::from(roundtripped_project.clone());
        let project = Rc::new(project);
        let sim = Simulation::new(&project, "main").unwrap();
        let compiled = sim.compile();
        assert!(compiled.is_ok());
        let compiled_sim = compiled.unwrap();
        let mut vm = Vm::new(compiled_sim).unwrap();
        vm.run_to_end().unwrap();
        (roundtripped_project, vm.into_results())
    };
    ensure_results(&expected, &results4);

    // finally ensure that if we re-serialize to XMILE the results are
    // byte-for-byte identical (we aren't losing any information)
    let serialized_xmile2 = xmile::project_to_xmile(&roundtripped_project).unwrap();
    assert_eq!(&serialized_xmile, &serialized_xmile2);
}

/// Interpreter-only simulation test - runs the interpreter and compares
/// results against expected output, but skips the VM (for models that use
/// array builtins like SUM which aren't yet supported in bytecode).
fn simulate_path_interpreter_only(xmile_path: &str) {
    eprintln!("model (interpreter-only): {xmile_path}");

    let f = File::open(xmile_path).unwrap();
    let mut f = BufReader::new(f);

    let datamodel_project = xmile::project_from_reader(&mut f);
    if let Err(ref err) = datamodel_project {
        eprintln!("model '{xmile_path}' error: {err}");
    }
    let datamodel_project = datamodel_project.unwrap();
    let project = Rc::new(Project::from(datamodel_project));
    let sim = Simulation::new(&project, "main").unwrap();

    let results = sim.run_to_end();
    assert!(results.is_ok(), "interpreter run failed: {:?}", results);
    let results = results.unwrap();

    // compare against expected results
    let expected = load_expected_results(xmile_path).unwrap();
    ensure_results(&expected, &results);
}

fn load_expected_results_for_mdl(mdl_path: &str) -> Option<Results> {
    let mdl_name = std::path::Path::new(mdl_path).file_name().unwrap();
    let dir_path = &mdl_path[0..(mdl_path.len() - mdl_name.len())];
    let dir_path = std::path::Path::new(dir_path);

    for (output_file, delimiter) in OUTPUT_FILES.iter() {
        let output_path = dir_path.join(output_file);
        if !output_path.exists() {
            continue;
        }
        return Some(load_csv(&output_path.to_string_lossy(), *delimiter).unwrap());
    }

    let dat_file = mdl_path.replace(".mdl", ".dat");
    let dat_path = std::path::Path::new(&dat_file);
    if dat_path.exists() {
        return Some(load_dat(&dat_file).unwrap());
    }

    None
}

/// Simulate a Vensim MDL file via the native parser, running both interpreter
/// and VM and comparing against expected output.
#[allow(dead_code)]
fn simulate_mdl_path(mdl_path: &str) {
    eprintln!("model (vensim mdl): {mdl_path}");

    let contents = std::fs::read_to_string(mdl_path)
        .unwrap_or_else(|e| panic!("failed to read {mdl_path}: {e}"));

    let datamodel_project =
        open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {mdl_path}: {e}"));
    let project = Rc::new(Project::from(datamodel_project.clone()));

    let sim = Simulation::new(&project, "main")
        .unwrap_or_else(|e| panic!("failed to create simulation for {mdl_path}: {e}"));

    let results1 = sim
        .run_to_end()
        .unwrap_or_else(|e| panic!("interpreter run failed for {mdl_path}: {e}"));

    let compiled = sim
        .compile()
        .unwrap_or_else(|e| panic!("compilation failed for {mdl_path}: {e}"));
    let mut vm =
        Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed for {mdl_path}: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed for {mdl_path}: {e}"));
    let results2 = vm.into_results();

    ensure_results(&results1, &results2);

    if let Some(expected) = load_expected_results_for_mdl(mdl_path) {
        ensure_results(&expected, &results1);
        ensure_results(&expected, &results2);
    }
}

/// Interpreter-only simulation test for MDL files (for models that use
/// array builtins like SUM which aren't yet supported in bytecode).
fn simulate_mdl_path_interpreter_only(mdl_path: &str) {
    eprintln!("model (vensim mdl, interpreter-only): {mdl_path}");

    let contents = std::fs::read_to_string(mdl_path)
        .unwrap_or_else(|e| panic!("failed to read {mdl_path}: {e}"));

    let datamodel_project =
        open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {mdl_path}: {e}"));
    let project = Rc::new(Project::from(datamodel_project));

    let sim = Simulation::new(&project, "main")
        .unwrap_or_else(|e| panic!("failed to create simulation for {mdl_path}: {e}"));

    let results = sim
        .run_to_end()
        .unwrap_or_else(|e| panic!("interpreter run failed for {mdl_path}: {e}"));

    if let Some(expected) = load_expected_results_for_mdl(mdl_path) {
        ensure_results(&expected, &results);
    }
}

/// Simulate a Vensim MDL file that references external data files.
/// Uses FilesystemDataProvider to resolve GET DIRECT references.
#[cfg(feature = "file_io")]
fn simulate_mdl_path_with_data(mdl_path: &str) {
    eprintln!("model (vensim mdl with data): {mdl_path}");

    let mdl_abs = std::path::Path::new(mdl_path);
    let model_dir = mdl_abs
        .parent()
        .unwrap_or_else(|| panic!("no parent dir for {mdl_path}"));

    let contents = std::fs::read_to_string(mdl_path)
        .unwrap_or_else(|e| panic!("failed to read {mdl_path}: {e}"));

    let provider = FilesystemDataProvider::new(model_dir);
    let datamodel_project = open_vensim_with_data(&contents, Some(&provider))
        .unwrap_or_else(|e| panic!("failed to parse {mdl_path}: {e}"));
    let project = Rc::new(Project::from(datamodel_project.clone()));

    let sim = Simulation::new(&project, "main")
        .unwrap_or_else(|e| panic!("failed to create simulation for {mdl_path}: {e}"));

    let results1 = sim
        .run_to_end()
        .unwrap_or_else(|e| panic!("interpreter run failed for {mdl_path}: {e}"));

    let compiled = sim
        .compile()
        .unwrap_or_else(|e| panic!("compilation failed for {mdl_path}: {e}"));
    let mut vm =
        Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed for {mdl_path}: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed for {mdl_path}: {e}"));
    let results2 = vm.into_results();

    ensure_results(&results1, &results2);

    if let Some(expected) = load_expected_results_for_mdl(mdl_path) {
        ensure_results(&expected, &results1);
        ensure_results(&expected, &results2);
    }
}

/// Like simulate_mdl_path_with_data, but interpreter-only (for models using
/// array builtins not yet implemented in the VM bytecode compiler).
#[cfg(feature = "file_io")]
#[allow(dead_code)]
fn simulate_mdl_path_with_data_interpreter_only(mdl_path: &str) {
    eprintln!("model (vensim mdl with data, interpreter-only): {mdl_path}");

    let mdl_abs = std::path::Path::new(mdl_path);
    let model_dir = mdl_abs
        .parent()
        .unwrap_or_else(|| panic!("no parent dir for {mdl_path}"));

    let contents = std::fs::read_to_string(mdl_path)
        .unwrap_or_else(|e| panic!("failed to read {mdl_path}: {e}"));

    let provider = FilesystemDataProvider::new(model_dir);
    let datamodel_project = open_vensim_with_data(&contents, Some(&provider))
        .unwrap_or_else(|e| panic!("failed to parse {mdl_path}: {e}"));
    let project = Rc::new(Project::from(datamodel_project));

    let sim = Simulation::new(&project, "main")
        .unwrap_or_else(|e| panic!("failed to create simulation for {mdl_path}: {e}"));

    let results = sim
        .run_to_end()
        .unwrap_or_else(|e| panic!("interpreter run failed for {mdl_path}: {e}"));

    if let Some(expected) = load_expected_results_for_mdl(mdl_path) {
        ensure_results(&expected, &results);
    }
}

#[test]
fn simulates_models_correctly() {
    for &path in TEST_MODELS {
        let file_path = format!("../../{path}");
        simulate_path(file_path.as_str());
    }
}

#[test]
fn simulates_aliases() {
    simulate_path("../../test/alias1/alias1.stmx");
}

#[test]
fn simulates_init_builtin() {
    simulate_path("../../test/builtin_init/builtin_init.stmx");
}

#[test]
fn simulates_arrays() {
    simulate_path("../../test/arrays1/arrays.stmx");
}

#[test]
fn simulates_array_sum_simple() {
    simulate_path("../../test/array_sum_simple/array_sum_simple.xmile");
}

#[test]
fn simulates_array_sum_expr() {
    simulate_path("../../test/array_sum_expr/array_sum_expr.xmile");
}

#[test]
fn simulates_array_multi_source() {
    // Tests multi-array expressions like SUM(a[*] + b[*])
    // This exercises the LoadIterViewTop opcode which loads from each array's
    // own view rather than a shared iteration view.
    simulate_path("../../test/array_multi_source/array_multi_source.xmile");
}

#[test]
fn simulates_array_broadcast() {
    // Tests cross-dimension broadcasting like sales[Region,Product] * price[Region]
    // where price is broadcast over the Product dimension.
    // This verifies that dimension IDs are correctly matched during iteration.
    simulate_path("../../test/array_broadcast/array_broadcast.xmile");
}

#[test]
fn simulates_modules() {
    simulate_path("../../test/modules_hares_and_foxes/modules_hares_and_foxes.stmx");
}

#[test]
fn simulates_modules2() {
    simulate_path("../../test/modules2/modules2.xmile");
}

#[test]
fn simulates_circular_dep_1() {
    simulate_path("../../test/circular-dep-1/model.stmx");
}

#[test]
fn simulates_previous() {
    simulate_path("../../test/previous/model.stmx");
}

#[test]
fn simulates_modules_with_complex_idents() {
    simulate_path("../../test/modules_with_complex_idents/modules_with_complex_idents.stmx");
}

#[test]
fn simulates_step_into_smth1() {
    simulate_path("../../test/step_into_smth1/model.stmx");
}

#[test]
fn simulates_subscript_index_name_values() {
    simulate_path("../../test/subscript_index_name_values/model.stmx");
}

#[test]
fn simulates_active_initial() {
    simulate_path("../../test/sdeverywhere/models/active_initial/active_initial.xmile");
}

#[test]
fn simulates_lookup() {
    simulate_path("../../test/sdeverywhere/models/lookup/lookup.xmile");
}

// Ignored: xmutil drops EXCEPT semantics and subscript mappings when converting
// MDL to XMILE. The XMILE file has incorrect/incomplete equations.
#[test]
#[ignore]
fn simulates_except_xmile() {
    simulate_path("../../test/sdeverywhere/models/except/except.xmile");
}

// Ignored: the except/except2 models use cross-dimension subscript mappings
// (DimD -> DimA) causing "output missing variable" errors -- the simulation
// completes but output variable names don't match the expected .dat names
// due to unresolved dimension mapping. EXCEPT parsing and compilation work;
// the basic EXCEPT test (simulates_except_basic_mdl) verifies correctness.
#[test]
#[ignore]
fn simulates_except() {
    simulate_mdl_path_interpreter_only("../../test/sdeverywhere/models/except/except.mdl");
}

// Ignored: same cross-dimension mapping issue as simulates_except.
#[test]
#[ignore]
fn simulates_except2() {
    simulate_mdl_path_interpreter_only("../../test/sdeverywhere/models/except2/except2.mdl");
}

#[test]
fn simulates_sum() {
    simulate_path("../../test/sdeverywhere/models/sum/sum.xmile");
}

#[test]
fn simulates_sum_interpreter_only() {
    simulate_path_interpreter_only("../../test/sdeverywhere/models/sum/sum.xmile");
}

// Ignored: xmutil drops EXCEPT semantics and subscript mappings when converting
// MDL to XMILE.
#[test]
#[ignore]
fn simulates_except_xmile_interpreter_only() {
    simulate_path_interpreter_only("../../test/sdeverywhere/models/except/except.xmile");
}

/// End-to-end test for EXCEPT through the MDL->simulation pipeline.
/// Uses a model without cross-dimension mappings so it doesn't hit the
/// DimD->DimA mapping limitation that blocks the full except/except2 models.
#[test]
fn simulates_except_basic_mdl() {
    let mdl = "\
{UTF-8}
DimA: A1, A2, A3 ~~|
SubA: A2, A3 ~~|
g[DimA] :EXCEPT: [A1] = 7 ~~|
g[A1] = 10 ~~|
h[DimA] :EXCEPT: [SubA] = 8 ~~|
p[DimA] :EXCEPT: [A1] = 2 ~~|
p[A1] = 5 ~~|
s[A3] = 13 ~~|
s[SubA] :EXCEPT: [A3] = 14 ~~|
u[DimA] :EXCEPT: [A1] = 1 ~~|
u[A1] = 99 ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 1 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let datamodel_project =
        open_vensim(mdl).unwrap_or_else(|e| panic!("failed to parse except_basic mdl: {e}"));
    let project = Rc::new(Project::from(datamodel_project));

    let sim = Simulation::new(&project, "main")
        .unwrap_or_else(|e| panic!("failed to create simulation: {e}"));

    let results = sim
        .run_to_end()
        .unwrap_or_else(|e| panic!("interpreter run failed: {e}"));

    let get = |name: &str| -> f64 {
        let ident = simlin_engine::common::Ident::<simlin_engine::common::Canonical>::new(name);
        let off = results.offsets[&ident];
        results.iter().next().unwrap()[off]
    };

    // g[DimA] :EXCEPT: [A1] = 7, g[A1] = 10
    assert!((get("g[a1]") - 10.0).abs() < 1e-10, "g[A1] should be 10");
    assert!((get("g[a2]") - 7.0).abs() < 1e-10, "g[A2] should be 7");
    assert!((get("g[a3]") - 7.0).abs() < 1e-10, "g[A3] should be 7");

    // h[DimA] :EXCEPT: [SubA] = 8 (no overrides for A2, A3)
    assert!((get("h[a1]") - 8.0).abs() < 1e-10, "h[A1] should be 8");
    assert!(
        (get("h[a2]") - 0.0).abs() < 1e-10,
        "h[A2] should be 0 (undefined)"
    );
    assert!(
        (get("h[a3]") - 0.0).abs() < 1e-10,
        "h[A3] should be 0 (undefined)"
    );

    // p[DimA] :EXCEPT: [A1] = 2, p[A1] = 5
    assert!((get("p[a1]") - 5.0).abs() < 1e-10, "p[A1] should be 5");
    assert!((get("p[a2]") - 2.0).abs() < 1e-10, "p[A2] should be 2");
    assert!((get("p[a3]") - 2.0).abs() < 1e-10, "p[A3] should be 2");

    // s[A3] = 13, s[SubA] :EXCEPT: [A3] = 14 => s[A2]=14, s[A3]=13
    assert!((get("s[a2]") - 14.0).abs() < 1e-10, "s[A2] should be 14");
    assert!((get("s[a3]") - 13.0).abs() < 1e-10, "s[A3] should be 13");

    // u[DimA] :EXCEPT: [A1] = 1, u[A1] = 99
    assert!((get("u[a1]") - 99.0).abs() < 1e-10, "u[A1] should be 99");
    assert!((get("u[a2]") - 1.0).abs() < 1e-10, "u[A2] should be 1");
    assert!((get("u[a3]") - 1.0).abs() < 1e-10, "u[A3] should be 1");

    // Also verify VM path works
    let compiled = sim
        .compile()
        .unwrap_or_else(|e| panic!("compilation failed: {e}"));
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed: {e}"));
    let vm_results = vm.into_results();

    let get_vm = |name: &str| -> f64 {
        let ident = simlin_engine::common::Ident::<simlin_engine::common::Canonical>::new(name);
        let off = vm_results.offsets[&ident];
        vm_results.iter().next().unwrap()[off]
    };

    assert!(
        (get_vm("g[a1]") - 10.0).abs() < 1e-10,
        "VM g[A1] should be 10"
    );
    assert!(
        (get_vm("g[a2]") - 7.0).abs() < 1e-10,
        "VM g[A2] should be 7"
    );
    assert!(
        (get_vm("p[a1]") - 5.0).abs() < 1e-10,
        "VM p[A1] should be 5"
    );
    assert!(
        (get_vm("s[a2]") - 14.0).abs() < 1e-10,
        "VM s[A2] should be 14"
    );
    assert!(
        (get_vm("s[a3]") - 13.0).abs() < 1e-10,
        "VM s[A3] should be 13"
    );
}

#[test]
fn simulates_2d_array() {
    simulate_path(
        "../../test/test-models/tests/subscript_2d_arrays/test_subscript_2d_arrays.xmile",
    );
}

// Commented out: test_generator approach is useful for discovery but generates many tests.
// Use simulates_arrayed_models_correctly below for the curated list.
// #[test_generator::test_resources("test/sdeverywhere/models/**/*.xmile")]
// fn simulates_sdeverywhere(resource: &str) {
//     let resource = format!("../../{}", resource);
//     simulate_path(&resource);
// }

/// SDEverywhere test models from test/sdeverywhere/models/**/*.xmile
/// These are Vensim models converted to XMILE format.
static TEST_SDEVERYWHERE_MODELS: &[&str] = &[
    // Passing tests
    "test/sdeverywhere/models/active_initial/active_initial.xmile",
    "test/sdeverywhere/models/comments/comments.xmile",
    "test/sdeverywhere/models/delay/delay.xmile",
    "test/sdeverywhere/models/elmcount/elmcount.xmile",
    "test/sdeverywhere/models/index/index.xmile",
    "test/sdeverywhere/models/initial/initial.xmile",
    "test/sdeverywhere/models/lookup/lookup.xmile",
    "test/sdeverywhere/models/pulsetrain/pulsetrain.xmile",
    "test/sdeverywhere/models/sir/sir.xmile",
    "test/sdeverywhere/models/smooth/smooth.xmile",
    "test/sdeverywhere/models/smooth3/smooth3.xmile",
    "test/sdeverywhere/models/specialchars/specialchars.xmile",
    "test/sdeverywhere/models/subalias/subalias.xmile",
    "test/sdeverywhere/models/trend/trend.xmile",
    //
    // xmutil strips GET DIRECT CONSTANTS data during conversion. Tested via MDL path.
    // "test/sdeverywhere/models/directconst/directconst.xmile",
    "test/sdeverywhere/models/longeqns/longeqns.xmile",
    "test/sdeverywhere/models/npv/npv.xmile",
    "test/sdeverywhere/models/sample/sample.xmile",
    "test/sdeverywhere/models/sum/sum.xmile",
    //
    // --- XMILE-path limitations (xmutil conversion issues) ---
    //
    // Tested via simulates_allocate_xmile (interpreter-only; VM lacks ALLOCATE AVAILABLE)
    // "test/sdeverywhere/models/allocate/allocate.xmile",
    //
    // xmutil converts DELAY FIXED into delay1 approximation which produces NaN.
    // Tested via MDL path.
    // "test/sdeverywhere/models/delayfixed/delayfixed.xmile",
    // "test/sdeverywhere/models/delayfixed2/delayfixed2.xmile",
    //
    // xmutil strips GET DIRECT CONSTANTS during XMILE conversion, leaving
    // empty equations. Tested via the MDL path.
    // "test/sdeverywhere/models/arrays_cname/arrays_cname.xmile",
    // "test/sdeverywhere/models/arrays_varname/arrays_varname.xmile",
    //
    // xmutil strips GET DIRECT DATA during conversion, leaving empty equations.
    // Tested via the MDL path (simulates_directdata_mdl etc.).
    // "test/sdeverywhere/models/directdata/directdata.xmile",
    //
    // xmutil strips GET DIRECT LOOKUPS during conversion
    // "test/sdeverywhere/models/directlookups/directlookups.xmile",
    //
    // xmutil strips GET DIRECT SUBSCRIPT during conversion
    // "test/sdeverywhere/models/directsubs/directsubs.xmile",
    //
    // xmutil drops EXCEPT semantics and subscript mappings in XMILE conversion.
    // These models are tested via the MDL path (simulates_except, simulates_except2).
    // "test/sdeverywhere/models/except/except.xmile",
    // "test/sdeverywhere/models/except2/except2.xmile",
    //
    // xmutil strips GET XLS DATA during conversion. Tested via MDL path.
    // "test/sdeverywhere/models/extdata/extdata.xmile",
    //
    // xmutil strips GET DATA BETWEEN TIMES calls, leaving broken data variable
    // references. Tested via MDL path.
    // "test/sdeverywhere/models/getdata/getdata.xmile",
    //
    // xmutil drops subscript mappings in XMILE conversion.
    // Tested via MDL path.
    // "test/sdeverywhere/models/mapping/mapping.xmile",
    // "test/sdeverywhere/models/multimap/multimap.xmile",
    //
    // xmutil doesn't inline external data from prune_data.dat into XMILE.
    // Tested via MDL path.
    // "test/sdeverywhere/models/prune/prune.xmile",
    //
    // xmutil expands QUANTUM(x,q) -> (q)*INT((x)/(q)), but INT is floor
    // per XMILE spec while Vensim INTEGER is truncation-toward-zero.
    // This gives wrong results for negative inputs. Tested via MDL path.
    // "test/sdeverywhere/models/quantum/quantum.xmile",
    //
    // xmutil drops subscript mappings. Tested via MDL path.
    // "test/sdeverywhere/models/subscript/subscript.xmile",
    //
    // --- Engine limitations ---
    //
    // NotSimulatable: element-level circular dependency (ce[t2] depends on ecc[t1],
    // ecc[t1] depends on ce[t1]) -- requires element-level dependency resolution
    // "test/sdeverywhere/models/ref/ref.xmile",
    //
    // NotSimulatable: element-level circular dependency via both XMILE and MDL paths
    // "test/sdeverywhere/models/interleaved/interleaved.xmile",
    //
    // Data variable loading: A Values uses Vensim's implicit companion .dat file
    // loading (not GET DIRECT), which isn't yet supported. The SUM(IF ...) pattern
    // is covered by array_tests::sum_of_conditional_tests.
    // "test/sdeverywhere/models/sumif/sumif.xmile",
    //
    // MismatchedDimensions: VECTOR ELM MAP cross-dimension indexing (b[B1] in DimA
    // context). The vector_simple subset is tested via simulates_vector_simple_mdl.
    // "test/sdeverywhere/models/vector/vector.xmile",
    //
    // --- Permanently excluded (not test models) ---
    //
    // Preprocessing test files with no simulation output
    // "test/sdeverywhere/models/flatten/expected.xmile",
    // "test/sdeverywhere/models/flatten/input1.xmile",
    // "test/sdeverywhere/models/flatten/input2.xmile",
    // "test/sdeverywhere/models/preprocess/expected.xmile",
    // "test/sdeverywhere/models/preprocess/input.xmile",
    //
    // Nested model directory duplicate, no .dat
    // "test/sdeverywhere/models/sir/model/sir.xmile",
];

#[test]
fn simulates_arrayed_models_correctly() {
    for &path in TEST_SDEVERYWHERE_MODELS {
        let file_path = format!("../../{path}");
        simulate_path(file_path.as_str());
    }
}

#[test]
fn simulates_lookup_arrayed() {
    simulate_path("../../test/lookup_arrayed/lookup_arrayed.xmile");
}

#[test]
fn simulates_delay_arrayed() {
    simulate_path("../../test/sdeverywhere/models/delay/delay.xmile");
}

#[test]
fn simulates_smooth3() {
    simulate_path("../../test/sdeverywhere/models/smooth3/smooth3.xmile");
}

/// Test for arrayed SMOOTH with dimension mappings.
/// This test is expected to fail until we implement dimension mapping support.
/// The Vensim model has mappings like `DimA: A1, A2, A3 -> DimB` which define
/// how elements of one dimension correspond to elements of another.
/// Variables like `s6[DimB] = SMOOTH(input_3[DimA], delay_3[DimA])` require
/// this mapping to know that A1->B1, A2->B2, A3->B3.
#[test]
fn simulates_smooth_with_dim_mappings() {
    simulate_path("../../test/sdeverywhere/models/smooth/smooth.xmile");
}

#[test]
fn simulates_subscript_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/subscript/subscript.mdl");
}

#[test]
fn simulates_mapping_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/mapping/mapping.mdl");
}

#[test]
fn simulates_multimap_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/multimap/multimap.mdl");
}

#[test]
fn simulates_npv_xmile() {
    simulate_path("../../test/sdeverywhere/models/npv/npv.xmile");
}

#[test]
fn simulates_npv_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/npv/npv.mdl");
}

// DELAY FIXED requires ring-buffer (pipeline delay) semantics, not
// exponential smoothing (delay1).  Currently mapped to delay1 as a rough
// approximation; these tests are ignored until VM-level ring buffer state
// is implemented.
#[test]
#[ignore]
fn simulates_delayfixed_xmile() {
    simulate_path("../../test/sdeverywhere/models/delayfixed/delayfixed.xmile");
}

#[test]
#[ignore]
fn simulates_delayfixed_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/delayfixed/delayfixed.mdl");
}

#[test]
#[ignore]
fn simulates_delayfixed2_xmile() {
    simulate_path("../../test/sdeverywhere/models/delayfixed2/delayfixed2.xmile");
}

#[test]
#[ignore]
fn simulates_delayfixed2_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/delayfixed2/delayfixed2.mdl");
}

#[test]
fn simulates_sample_xmile() {
    simulate_path("../../test/sdeverywhere/models/sample/sample.xmile");
}

#[test]
fn simulates_sample_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/sample/sample.mdl");
}

#[test]
fn simulates_quantum_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/quantum/quantum.mdl");
}

#[test]
fn simulates_vector_simple_mdl() {
    simulate_mdl_path_interpreter_only(
        "../../test/sdeverywhere/models/vector_simple/vector_simple.mdl",
    );
}

#[test]
fn simulates_allocate_mdl() {
    simulate_mdl_path_interpreter_only("../../test/sdeverywhere/models/allocate/allocate.mdl");
}

#[test]
fn simulates_allocate_xmile() {
    simulate_path_interpreter_only("../../test/sdeverywhere/models/allocate/allocate.xmile");
}

#[test]
fn simulates_longeqns_mdl() {
    simulate_mdl_path_interpreter_only("../../test/sdeverywhere/models/longeqns/longeqns.mdl");
}

// Ignored: the XMILE path is broken (xmutil strips GET DATA BETWEEN TIMES to
// zeroed-out equations). The MDL path requires external .dat file loading for
// data variables, which is not yet fully supported.
#[test]
#[ignore]
fn simulates_getdata_xmile() {
    simulate_path("../../test/sdeverywhere/models/getdata/getdata.xmile");
}

#[test]
#[ignore]
fn simulates_getdata_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/getdata/getdata.mdl");
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

#[test]
fn verifies_ai_information_generated_then_edited() {
    verify_ai_information("../../test/ai-information/GeneratedByAIThenEdited.stmx");
}

#[test]
fn verifies_ai_information_pure_ai() {
    verify_ai_information("../../test/ai-information/PureAIModel.stmx");
}

#[test]
fn verifies_ai_information_pure_human() {
    verify_ai_information("../../test/ai-information/PureHumanModel.stmx");
}

#[test]
fn verifies_ai_information_with_modules_and_arrays() {
    verify_ai_information("../../test/ai-information/WithModulesAndArrays.stmx");
}

fn verify_ai_information(xmile_path: &str) {
    let known_keys = HashMap::from([(
        "https://iseesystems.com/keys/stella01.txt",
        "AAAAC3NzaC1lZDI1NTE5AAAAIP5Rg+bCssFIB2b2F9H/lUhVBXwtrBCtyRgiiq9RYkXS",
    )]);

    eprintln!("model: {xmile_path}");

    let f = File::open(xmile_path).unwrap();
    let mut f = BufReader::new(f);

    let datamodel_project = xmile::project_from_reader(&mut f);
    if let Err(ref err) = datamodel_project {
        eprintln!("model '{xmile_path}' error: {err}");
    }

    #[allow(unused_variables)]
    let datamodel_project = datamodel_project.unwrap();

    let ai_info = datamodel_project.ai_information.as_ref().unwrap();
    let key_bytes_encoded = known_keys[ai_info.status.key_url.as_str()];

    use base64::{Engine as _, engine::general_purpose};
    let key_bytes = general_purpose::STANDARD.decode(key_bytes_encoded).unwrap();

    let openssh_pubkey = ssh_key::PublicKey::from_bytes(&key_bytes).unwrap();
    let raw_pubkey = openssh_pubkey.key_data().ed25519().unwrap();

    // OpenSSH format: skip the first 19 bytes to get to the actual 32-byte Ed25519 key
    let key = ed25519_dalek::VerifyingKey::from_bytes(&raw_pubkey.0).unwrap();

    simlin_engine::ai_info::verify(&datamodel_project, &key).unwrap()
}

#[test]
fn simulates_wrld3_03() {
    let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";

    eprintln!("model (vensim mdl): {mdl_path}");

    let contents = std::fs::read_to_string(mdl_path)
        .unwrap_or_else(|e| panic!("failed to read {mdl_path}: {e}"));

    let datamodel_project =
        open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {mdl_path}: {e}"));
    let project = Rc::new(Project::from(datamodel_project.clone()));

    let sim = Simulation::new(&project, "main")
        .unwrap_or_else(|e| panic!("failed to create simulation for {mdl_path}: {e}"));

    let results1 = sim
        .run_to_end()
        .unwrap_or_else(|e| panic!("interpreter run failed for {mdl_path}: {e}"));

    let compiled = sim
        .compile()
        .unwrap_or_else(|e| panic!("compilation failed for {mdl_path}: {e}"));
    let mut vm =
        Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed for {mdl_path}: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed for {mdl_path}: {e}"));
    let results2 = vm.into_results();

    ensure_results(&results1, &results2);

    // Compare simulation output against the Vensim reference VDF data.
    // Uses empirical matching (time series correlation) to map VDF entries
    // to simulation variables, since the VDF metadata chain linking names
    // to data entries has not been fully decoded.
    let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";
    let vdf_data_bytes =
        std::fs::read(vdf_path).unwrap_or_else(|e| panic!("failed to read {vdf_path}: {e}"));
    let vdf_file = simlin_engine::vdf::VdfFile::parse(vdf_data_bytes)
        .unwrap_or_else(|e| panic!("failed to parse VDF {vdf_path}: {e}"));

    let vdf_results = vdf_file
        .to_results(&results1)
        .unwrap_or_else(|e| panic!("VDF to_results failed: {e}"));

    // Cross-simulator comparison needs wider tolerance than our own
    // interpreter vs VM check: Vensim's integration may differ slightly,
    // and VDF stores f32 values (~7 significant digits).
    ensure_vdf_results(&vdf_results, &results1);
}

// C-LEARN uses Vensim macros (SAMPLE UNTIL, SSHAPE) that the native MDL
// parser reads but cannot yet expand/inline into the model. Without macro
// expansion, the macro-generated variables appear as UnknownBuiltin errors
// and the model is NotSimulatable. Macro expansion is a significant new
// feature -- parsing is complete but conversion/inlining is not implemented.
#[test]
#[ignore]
fn simulates_clearn() {
    let mdl_path = "../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl";

    eprintln!("model (vensim mdl): {mdl_path}");

    let contents = std::fs::read_to_string(mdl_path)
        .unwrap_or_else(|e| panic!("failed to read {mdl_path}: {e}"));

    let datamodel_project =
        open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {mdl_path}: {e}"));
    let project = Rc::new(Project::from(datamodel_project.clone()));

    let sim = Simulation::new(&project, "main")
        .unwrap_or_else(|e| panic!("failed to create simulation for {mdl_path}: {e}"));

    let results1 = sim
        .run_to_end()
        .unwrap_or_else(|e| panic!("interpreter run failed for {mdl_path}: {e}"));

    let compiled = sim
        .compile()
        .unwrap_or_else(|e| panic!("compilation failed for {mdl_path}: {e}"));
    let mut vm =
        Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed for {mdl_path}: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed for {mdl_path}: {e}"));
    let results2 = vm.into_results();

    ensure_results(&results1, &results2);

    let vdf_path = "../../test/xmutil_test_models/Ref.vdf";
    let vdf_data_bytes =
        std::fs::read(vdf_path).unwrap_or_else(|e| panic!("failed to read {vdf_path}: {e}"));
    let vdf_file = simlin_engine::vdf::VdfFile::parse(vdf_data_bytes)
        .unwrap_or_else(|e| panic!("failed to parse VDF {vdf_path}: {e}"));

    let vdf_results = vdf_file
        .to_results(&results1)
        .unwrap_or_else(|e| panic!("VDF to_results failed: {e}"));

    ensure_vdf_results(&vdf_results, &results1);
}

/// All test models that the monolithic compiler can handle.
/// The incremental path must also handle these.
static ALL_INCREMENTALLY_COMPILABLE_MODELS: &[&str] = &[
    "../../test/alias1/alias1.stmx",
    "../../test/builtin_init/builtin_init.stmx",
    "../../test/arrays1/arrays.stmx",
    "../../test/array_sum_simple/array_sum_simple.xmile",
    "../../test/array_sum_expr/array_sum_expr.xmile",
    "../../test/array_multi_source/array_multi_source.xmile",
    "../../test/array_broadcast/array_broadcast.xmile",
    "../../test/modules_hares_and_foxes/modules_hares_and_foxes.stmx",
    "../../test/modules2/modules2.xmile",
    "../../test/circular-dep-1/model.stmx",
    "../../test/previous/model.stmx",
    "../../test/modules_with_complex_idents/modules_with_complex_idents.stmx",
    "../../test/step_into_smth1/model.stmx",
    "../../test/subscript_index_name_values/model.stmx",
    "../../test/sdeverywhere/models/active_initial/active_initial.xmile",
    "../../test/sdeverywhere/models/lookup/lookup.xmile",
    "../../test/sdeverywhere/models/sum/sum.xmile",
    "../../test/sdeverywhere/models/delay/delay.xmile",
    "../../test/sdeverywhere/models/smooth3/smooth3.xmile",
    "../../test/sdeverywhere/models/smooth/smooth.xmile",
    "../../test/lookup_arrayed/lookup_arrayed.xmile",
];

/// Verify that the salsa-based incremental compilation path successfully
/// compiles every test model that the monolithic path handles.
#[cfg(feature = "file_io")]
#[test]
fn incremental_compilation_covers_all_models() {
    use simlin_engine::db;

    let mut failures: Vec<(String, String)> = Vec::new();

    for model_path in ALL_INCREMENTALLY_COMPILABLE_MODELS
        .iter()
        .chain(TEST_MODELS.iter())
    {
        let f = match File::open(model_path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let mut f = BufReader::new(f);

        let datamodel_project = if model_path.ends_with(".stmx") || model_path.ends_with(".xmile") {
            match xmile::project_from_reader(&mut f) {
                Ok(p) => p,
                Err(_) => continue,
            }
        } else {
            continue;
        };

        let model_path_owned = model_path.to_string();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut salsa_db = db::SimlinDb::default();
            let sync_state =
                db::sync_from_datamodel_incremental(&mut salsa_db, &datamodel_project, None);
            let sync = sync_state.to_sync_result();
            db::compile_project_incremental(&salsa_db, sync.project, "main")
        }));

        match result {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                failures.push((model_path_owned, format!("{e}")));
            }
            Err(panic) => {
                let msg = if let Some(s) = panic.downcast_ref::<String>() {
                    s.clone()
                } else if let Some(s) = panic.downcast_ref::<&str>() {
                    s.to_string()
                } else {
                    "unknown panic".to_string()
                };
                failures.push((model_path_owned, format!("PANIC: {msg}")));
            }
        }
    }

    if !failures.is_empty() {
        eprintln!("\nIncremental compilation failures:");
        for (model, err) in &failures {
            eprintln!("  {model}: {err}");
        }
        panic!(
            "{} of {} models failed incremental compilation",
            failures.len(),
            ALL_INCREMENTALLY_COMPILABLE_MODELS.len() + TEST_MODELS.len(),
        );
    }
}

// -- External data model tests (MDL path with FilesystemDataProvider) --

// Ignored: requires Excel data support AND dimension equivalences (DimC <-> DimM)
#[cfg(feature = "ext_data")]
#[test]
#[ignore]
fn simulates_directdata_mdl() {
    simulate_mdl_path_with_data("../../test/sdeverywhere/models/directdata/directdata.mdl");
}

// Ignored: requires arrayed GET DIRECT CONSTANTS (B2* pattern) and EXCEPT support
#[test]
#[ignore]
fn simulates_directconst_mdl() {
    simulate_mdl_path_with_data("../../test/sdeverywhere/models/directconst/directconst.mdl");
}

// Ignored: requires arrayed GET DIRECT LOOKUPS with row-oriented addressing
#[test]
#[ignore]
fn simulates_directlookups_mdl() {
    simulate_mdl_path_with_data("../../test/sdeverywhere/models/directlookups/directlookups.mdl");
}

// Ignored: requires cross-dimension mapping (DimA -> DimB, DimC)
#[test]
#[ignore]
fn simulates_directsubs_mdl() {
    simulate_mdl_path_with_data("../../test/sdeverywhere/models/directsubs/directsubs.mdl");
}

/// End-to-end test: scalar GET DIRECT DATA from CSV, parsed through MDL
/// pipeline with FilesystemDataProvider, simulated via interpreter and VM.
#[test]
fn simulates_get_direct_data_scalar_csv() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();

    // Write a simple CSV data file
    let csv_path = dir.path().join("scalar_data.csv");
    let mut f = std::fs::File::create(&csv_path).unwrap();
    write!(f, "Year,Value\n2000,100\n2010,200\n2020,300\n").unwrap();

    let mdl = "\
{UTF-8}
x := GET DIRECT DATA('scalar_data.csv', ',', 'A', 'B2') ~~|
y = x * 2 ~~|
INITIAL TIME = 2000 ~~|
FINAL TIME = 2020 ~~|
TIME STEP = 10 ~~|
SAVEPER = TIME STEP ~~|
";
    let provider = FilesystemDataProvider::new(dir.path());
    let datamodel_project = open_vensim_with_data(mdl, Some(&provider))
        .unwrap_or_else(|e| panic!("failed to parse: {e}"));
    let project = Rc::new(Project::from(datamodel_project));

    let sim = Simulation::new(&project, "main")
        .unwrap_or_else(|e| panic!("failed to create simulation: {e}"));

    let results = sim
        .run_to_end()
        .unwrap_or_else(|e| panic!("interpreter run failed: {e}"));

    let get = |name: &str| -> Vec<f64> {
        let ident = simlin_engine::common::Ident::<simlin_engine::common::Canonical>::new(name);
        let off = results.offsets[&ident];
        results.iter().map(|row| row[off]).collect()
    };

    let x_vals = get("x");
    assert_eq!(x_vals.len(), 3);
    assert!(
        (x_vals[0] - 100.0).abs() < 1e-6,
        "x at t=2000 should be 100"
    );
    assert!(
        (x_vals[1] - 200.0).abs() < 1e-6,
        "x at t=2010 should be 200"
    );
    assert!(
        (x_vals[2] - 300.0).abs() < 1e-6,
        "x at t=2020 should be 300"
    );

    let y_vals = get("y");
    assert!(
        (y_vals[0] - 200.0).abs() < 1e-6,
        "y at t=2000 should be 200"
    );
    assert!(
        (y_vals[2] - 600.0).abs() < 1e-6,
        "y at t=2020 should be 600"
    );

    // Also verify VM path works
    let compiled = sim
        .compile()
        .unwrap_or_else(|e| panic!("compilation failed: {e}"));
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed: {e}"));
    let vm_results = vm.into_results();
    ensure_results(&results, &vm_results);
}

/// End-to-end test: scalar GET DIRECT CONSTANTS from CSV.
#[test]
fn simulates_get_direct_constants_scalar_csv() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();

    let csv_path = dir.path().join("const_data.csv");
    let mut f = std::fs::File::create(&csv_path).unwrap();
    write!(f, "label,\n,42\n").unwrap();

    let mdl = "\
{UTF-8}
a = GET DIRECT CONSTANTS('const_data.csv', ',', 'B2') ~~|
b = a + 8 ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 1 ~~|
TIME STEP = 1 ~~|
SAVEPER = TIME STEP ~~|
";
    let provider = FilesystemDataProvider::new(dir.path());
    let datamodel_project = open_vensim_with_data(mdl, Some(&provider))
        .unwrap_or_else(|e| panic!("failed to parse: {e}"));
    let project = Rc::new(Project::from(datamodel_project));

    let sim = Simulation::new(&project, "main")
        .unwrap_or_else(|e| panic!("failed to create simulation: {e}"));

    let results = sim
        .run_to_end()
        .unwrap_or_else(|e| panic!("interpreter run failed: {e}"));

    let get = |name: &str| -> f64 {
        let ident = simlin_engine::common::Ident::<simlin_engine::common::Canonical>::new(name);
        let off = results.offsets[&ident];
        results.iter().next().unwrap()[off]
    };

    assert!((get("a") - 42.0).abs() < 1e-6, "a should be 42");
    assert!((get("b") - 50.0).abs() < 1e-6, "b should be 50");

    // Also verify VM path
    let compiled = sim.compile().unwrap();
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let vm_results = vm.into_results();
    ensure_results(&results, &vm_results);
}

/// End-to-end test: scalar GET DIRECT LOOKUPS from CSV.
#[test]
fn simulates_get_direct_lookups_scalar_csv() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();

    // CSV with time in column 1 and values in column 2:
    // row 1: header
    // row 2+: data pairs (x, y)
    let csv_path = dir.path().join("lookup_data.csv");
    let mut f = std::fs::File::create(&csv_path).unwrap();
    write!(f, "time,value\n0,10\n5,20\n10,30\n").unwrap();

    let mdl = "\
{UTF-8}
x := GET DIRECT LOOKUPS('lookup_data.csv', ',', 'A', 'B2') ~~|
y = x * 2 ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 10 ~~|
TIME STEP = 5 ~~|
SAVEPER = TIME STEP ~~|
";
    let provider = FilesystemDataProvider::new(dir.path());
    let datamodel_project = open_vensim_with_data(mdl, Some(&provider))
        .unwrap_or_else(|e| panic!("failed to parse: {e}"));
    let project = Rc::new(Project::from(datamodel_project));

    let sim = Simulation::new(&project, "main")
        .unwrap_or_else(|e| panic!("failed to create simulation: {e}"));

    let results = sim
        .run_to_end()
        .unwrap_or_else(|e| panic!("interpreter run failed: {e}"));

    let get = |name: &str, step: usize| -> f64 {
        let ident = simlin_engine::common::Ident::<simlin_engine::common::Canonical>::new(name);
        let off = results.offsets[&ident];
        results.iter().nth(step).unwrap()[off]
    };

    // At time 0: x=10, y=20
    assert!((get("x", 0) - 10.0).abs() < 1e-6, "x at t=0 should be 10");
    assert!((get("y", 0) - 20.0).abs() < 1e-6, "y at t=0 should be 20");
    // At time 5: x=20, y=40
    assert!((get("x", 1) - 20.0).abs() < 1e-6, "x at t=5 should be 20");
    assert!((get("y", 1) - 40.0).abs() < 1e-6, "y at t=5 should be 40");
    // At time 10: x=30, y=60
    assert!((get("x", 2) - 30.0).abs() < 1e-6, "x at t=10 should be 30");
    assert!((get("y", 2) - 60.0).abs() < 1e-6, "y at t=10 should be 60");

    // Also verify VM path
    let compiled = sim.compile().unwrap();
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let vm_results = vm.into_results();
    ensure_results(&results, &vm_results);
}
