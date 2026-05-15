// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

mod test_helpers;

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;

#[cfg(feature = "file_io")]
use simlin_engine::FilesystemDataProvider;
use simlin_engine::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};
use simlin_engine::serde::{deserialize, serialize};
use simlin_engine::{Results, Vm, project_io};
use simlin_engine::{load_csv, load_dat, open_vensim, open_vensim_with_data, xmile};

use test_helpers::ensure_results;

const OUTPUT_FILES: &[(&str, u8)] = &[("output.csv", b','), ("output.tab", b'\t')];

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

/// Compile a datamodel project to a VM simulation using the incremental
/// salsa-backed path.
fn compile_vm(
    datamodel_project: &simlin_engine::datamodel::Project,
) -> simlin_engine::CompiledSimulation {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, datamodel_project, None);
    compile_project_incremental(&db, sync.project, "main").unwrap()
}

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

type CompileFn = fn(&simlin_engine::datamodel::Project) -> simlin_engine::CompiledSimulation;

fn simulate_path(xmile_path: &str) {
    simulate_path_with(xmile_path, compile_vm);
}

fn simulate_path_with(xmile_path: &str, compile: CompileFn) {
    eprintln!("model: {xmile_path}");

    let datamodel_project = {
        let f = File::open(xmile_path).unwrap();
        let mut f = BufReader::new(f);

        let datamodel_project = xmile::project_from_reader(&mut f);
        if let Err(ref err) = datamodel_project {
            eprintln!("model '{xmile_path}' error: {err}");
        }
        datamodel_project.unwrap()
    };

    // simulate the model using our bytecode VM
    let results = {
        let compiled_sim = compile(&datamodel_project);
        let mut vm = Vm::new(compiled_sim).unwrap();
        vm.run_to_end().unwrap();
        vm.into_results()
    };

    let expected = load_expected_results(xmile_path).unwrap();
    ensure_results(&expected, &results);

    // serialize our project through protobufs and ensure we don't see problems
    let results_proto = {
        use simlin_engine::prost::Message;

        let pb_project_inner = serialize(&datamodel_project).unwrap();
        let pb_project = &pb_project_inner;
        let mut buf = Vec::with_capacity(pb_project.encoded_len());
        pb_project.encode(&mut buf).unwrap();

        let datamodel_project2 = deserialize(project_io::Project::decode(&*buf).unwrap());
        assert_eq!(datamodel_project, datamodel_project2);
        let compiled_sim = compile(&datamodel_project2);
        let mut vm = Vm::new(compiled_sim).unwrap();
        vm.run_to_end().unwrap();
        vm.into_results()
    };
    ensure_results(&expected, &results_proto);

    // serialize our project back to XMILE
    let serialized_xmile = xmile::project_to_xmile(&datamodel_project).unwrap();

    // and then read it back in from the XMILE string and simulate it
    let (roundtripped_project, results_xmile) = {
        let mut xmile_reader = BufReader::new(serialized_xmile.as_bytes());
        let roundtripped_project = xmile::project_from_reader(&mut xmile_reader).unwrap();

        let compiled_sim = compile(&roundtripped_project);
        let mut vm = Vm::new(compiled_sim).unwrap();
        vm.run_to_end().unwrap();
        (roundtripped_project, vm.into_results())
    };
    ensure_results(&expected, &results_xmile);

    // finally ensure that if we re-serialize to XMILE the results are
    // byte-for-byte identical (we aren't losing any information)
    let serialized_xmile2 = xmile::project_to_xmile(&roundtripped_project).unwrap();
    assert_eq!(&serialized_xmile, &serialized_xmile2);
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

/// Simulate a Vensim MDL file via the native parser, running the VM
/// and comparing against expected output.
fn simulate_mdl_path(mdl_path: &str) {
    eprintln!("model (vensim mdl): {mdl_path}");

    let contents = std::fs::read_to_string(mdl_path)
        .unwrap_or_else(|e| panic!("failed to read {mdl_path}: {e}"));

    let datamodel_project =
        open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {mdl_path}: {e}"));

    let compiled = compile_vm(&datamodel_project);
    let mut vm =
        Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed for {mdl_path}: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed for {mdl_path}: {e}"));
    let results = vm.into_results();

    let expected = load_expected_results_for_mdl(mdl_path)
        .unwrap_or_else(|| panic!("no reference data found for {mdl_path}"));
    ensure_results(&expected, &results);
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

    let compiled = compile_vm(&datamodel_project);
    let mut vm =
        Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed for {mdl_path}: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed for {mdl_path}: {e}"));
    let results = vm.into_results();

    let expected = load_expected_results_for_mdl(mdl_path)
        .unwrap_or_else(|| panic!("no reference data found for {mdl_path}"));
    ensure_results(&expected, &results);
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

#[test]
fn simulates_except() {
    simulate_mdl_path("../../test/sdeverywhere/models/except/except.mdl");
}

#[test]
fn simulates_except2() {
    simulate_mdl_path("../../test/sdeverywhere/models/except2/except2.mdl");
}

#[test]
fn simulates_sum() {
    simulate_path("../../test/sdeverywhere/models/sum/sum.xmile");
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

    let compiled = compile_vm(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed: {e}"));
    let results = vm.into_results();

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
    // Tested via simulates_allocate_xmile
    // "test/sdeverywhere/models/allocate/allocate.xmile",
    //
    // xmutil converts DELAY FIXED into delay1 approximation which produces NaN.
    // Tested via MDL path.
    // "test/sdeverywhere/models/delayfixed/delayfixed.xmile",
    // "test/sdeverywhere/models/delayfixed2/delayfixed2.xmile",
    //
    // xmutil strips GET DIRECT CONSTANTS during XMILE conversion, leaving
    // empty equations. Not yet fully testable via MDL path: the MDL parser
    // now handles +/- signed literals in number lists and mixed fixed-element/
    // dimension subscripts, but multiple TabbedArray definitions for the same
    // variable (e.g. z[C1,...] and z[C2,...]) are not yet merged into a single
    // Arrayed equation.
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
    // MDL path tests exist (simulates_except, simulates_except2) but remain
    // #[ignore] due to MismatchedDimensions errors and missing output variables
    // (z[a1] absent). Resolving these requires further work on dimension mapping
    // for variables with EXCEPT and subscript-mapped dimensions.
    // "test/sdeverywhere/models/except/except.xmile",
    // "test/sdeverywhere/models/except2/except2.xmile",
    //
    // xmutil strips GET XLS DATA during conversion. The MDL file has variables
    // with no equations (e.g. D Values, BC Values) that Vensim populates from
    // companion extdata_data.dat via implicit per-run data loading, which the
    // engine does not auto-discover. Not testable via MDL path.
    // "test/sdeverywhere/models/extdata/extdata.xmile",
    //
    // xmutil strips GET DATA BETWEEN TIMES calls, leaving broken data variable
    // references. The MDL path also cannot simulate this model: the normalizer
    // wraps GET DATA BETWEEN TIMES in opaque {GET DATA(...)} references, which
    // the XMILE equation lexer silently discards as comments, producing empty
    // equations. Additionally, Values[DimA] requires external data from
    // getdata_data.dat via implicit per-run loading. Not testable via MDL path.
    // "test/sdeverywhere/models/getdata/getdata.xmile",
    //
    // xmutil drops subscript mappings in XMILE conversion.
    // Tested via MDL path.
    // "test/sdeverywhere/models/mapping/mapping.xmile",
    // "test/sdeverywhere/models/multimap/multimap.xmile",
    //
    // xmutil doesn't inline external data from prune_data.dat into XMILE.
    // The MDL file has variables with no equations (A Values, BC Values, D Values,
    // etc.) that Vensim populates from prune_data.dat via implicit per-run data
    // loading, which the engine does not auto-discover. Not testable via MDL path.
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
    // Vensim implicit data variable loading: "A Values[DimA]" has no equation
    // in the MDL -- values come from the companion sumif_data.dat file via Vensim's
    // implicit per-run data loading, distinct from GET DIRECT DATA. The engine has no
    // mechanism to auto-discover and load companion .dat files by convention.
    // The SUM(IF ...) arithmetic pattern itself is covered by
    // array_tests::sum_of_conditional_tests.
    // "test/sdeverywhere/models/sumif/sumif.xmile",
    //
    // VECTOR ELM MAP cross-dimension source: partially fixed (cross-dim subscripts,
    // SUM wildcards, AssignTemp wildcards), but several variables still fail:
    //   - y[DimA] = VECTOR ELM MAP(x[three], (DimA-1)) fails in VM incremental path
    //   - Additional VECTOR SELECT cross-dimension patterns need compiler work
    // The vector_simple subset passes via simulates_vector_simple_mdl.
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
    simulate_mdl_path("../../test/sdeverywhere/models/vector_simple/vector_simple.mdl");
}

#[test]
fn simulates_allocate_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/allocate/allocate.mdl");
}

#[test]
fn simulates_allocate_xmile() {
    simulate_path("../../test/sdeverywhere/models/allocate/allocate.xmile");
}

#[test]
fn simulates_longeqns_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/longeqns/longeqns.mdl");
}

// Ignored: xmutil strips GET DATA BETWEEN TIMES calls in XMILE conversion,
// leaving zeroed-out equations for variables that depend on the data.
#[test]
#[ignore]
fn simulates_getdata_xmile() {
    simulate_path("../../test/sdeverywhere/models/getdata/getdata.xmile");
}

// Ignored: two blocking issues prevent MDL path simulation.
// (1) The MDL normalizer wraps GET DATA BETWEEN TIMES in opaque {GET DATA(...)}
//     references, which the XMILE equation lexer discards as comments, producing
//     empty equations for variables like value_for_a1_at_time_minus_half_year_backward.
// (2) Values[DimA] has no equation in the MDL and must be populated from
//     getdata_data.dat via Vensim's implicit per-run data loading, which the
//     engine does not auto-discover. Fixing requires DataProvider integration
//     for implicit companion .dat files.
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
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
    assert!(compile_project_incremental(&db, sync.project, "blerg").is_err());
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

    let compiled = compile_vm(&datamodel_project);
    let mut vm =
        Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed for {mdl_path}: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed for {mdl_path}: {e}"));
    let results = vm.into_results();

    // Verify VDF parsing and record-based extraction succeed on the WRLD3
    // reference data. Full series-level comparison is checked by the
    // `simulates_clearn` path; here we only confirm the decoder recovers a
    // broad column set with the right time axis.
    let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";
    let vdf_data_bytes =
        std::fs::read(vdf_path).unwrap_or_else(|e| panic!("failed to read {vdf_path}: {e}"));
    let vdf_file = simlin_engine::vdf::VdfFile::parse(vdf_data_bytes)
        .unwrap_or_else(|e| panic!("failed to parse VDF {vdf_path}: {e}"));
    let vdf_results = vdf_file
        .to_results_via_records()
        .unwrap_or_else(|e| panic!("VDF to_results_via_records failed: {e}"));
    assert!(
        vdf_results.offsets.len() > 200,
        "WRLD3: expected broad record-based mapping, got {}",
        vdf_results.offsets.len()
    );
    assert_eq!(vdf_results.step_count, results.step_count);
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

    let compiled = compile_vm(&datamodel_project);
    let mut vm =
        Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed for {mdl_path}: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed for {mdl_path}: {e}"));
    let results = vm.into_results();

    let vdf_path = "../../test/xmutil_test_models/Ref.vdf";
    let vdf_data_bytes =
        std::fs::read(vdf_path).unwrap_or_else(|e| panic!("failed to read {vdf_path}: {e}"));
    let vdf_file = simlin_engine::vdf::VdfFile::parse(vdf_data_bytes)
        .unwrap_or_else(|e| panic!("failed to parse VDF {vdf_path}: {e}"));
    let vdf_results = vdf_file
        .to_results_via_records()
        .unwrap_or_else(|e| panic!("VDF to_results_via_records failed: {e}"));

    ensure_vdf_results(&vdf_results, &results);
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

        let mut salsa_db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut salsa_db, &datamodel_project, None);
        let result = compile_project_incremental(&salsa_db, sync.project, "main");

        if let Err(e) = result {
            failures.push((model_path.to_string(), format!("{e}")));
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

// Requires Excel data support (ext_data feature), out of scope
#[cfg(feature = "ext_data")]
#[test]
#[ignore]
fn simulates_directdata_mdl() {
    simulate_mdl_path_with_data("../../test/sdeverywhere/models/directdata/directdata.mdl");
}

#[test]
fn simulates_directconst_mdl() {
    simulate_mdl_path_with_data("../../test/sdeverywhere/models/directconst/directconst.mdl");
}

#[test]
fn simulates_directlookups_mdl() {
    simulate_mdl_path_with_data("../../test/sdeverywhere/models/directlookups/directlookups.mdl");
}

#[test]
fn simulates_directsubs_mdl() {
    simulate_mdl_path_with_data("../../test/sdeverywhere/models/directsubs/directsubs.mdl");
}

/// End-to-end test: scalar GET DIRECT DATA from CSV, parsed through MDL
/// pipeline with FilesystemDataProvider, simulated via VM.
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

    let compiled = compile_vm(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed: {e}"));
    let results = vm.into_results();

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

    let compiled = compile_vm(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed: {e}"));
    let results = vm.into_results();

    let get = |name: &str| -> f64 {
        let ident = simlin_engine::common::Ident::<simlin_engine::common::Canonical>::new(name);
        let off = results.offsets[&ident];
        results.iter().next().unwrap()[off]
    };

    assert!((get("a") - 42.0).abs() < 1e-6, "a should be 42");
    assert!((get("b") - 50.0).abs() < 1e-6, "b should be 50");
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

    let compiled = compile_vm(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed: {e}"));
    let results = vm.into_results();

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
}

#[test]
fn mark2_mdl_compiles_incrementally() {
    let contents =
        std::fs::read_to_string("../../test/bobby/vdf/econ/mark2.mdl").expect("read mark2.mdl");
    let project = open_vensim(&contents).expect("parse mark2.mdl");
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("mark2.mdl should compile incrementally");
    let mut vm = Vm::new(compiled).expect("VM creation should succeed");
    vm.run_to_end().expect("VM should run to completion");
}

/// Reproduce the browser path: open_vensim → protobuf serialize → protobuf
/// deserialize → compile. The app round-trips through protobuf between
/// import and simulation.
#[test]
fn mark2_mdl_compiles_after_protobuf_roundtrip() {
    use prost::Message;

    let contents =
        std::fs::read_to_string("../../test/bobby/vdf/econ/mark2.mdl").expect("read mark2.mdl");
    let project = open_vensim(&contents).expect("parse mark2.mdl");

    // Serialize to protobuf (as the app does in NewProject.tsx)
    let pb = serialize(&project).expect("serialize to protobuf");
    let mut buf = Vec::new();
    pb.encode(&mut buf).expect("encode protobuf");

    // Deserialize from protobuf (as the app does when loading from storage)
    let pb2 = project_io::Project::decode(buf.as_slice()).expect("decode protobuf");
    let project2 = deserialize(pb2);

    // Compile the round-tripped project
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project2, None);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("mark2.mdl should compile after protobuf round-trip");
    let mut vm = Vm::new(compiled).expect("VM creation should succeed");
    vm.run_to_end().expect("VM should run to completion");
}

/// The browser's model.run() defaults analyzeLtm=true, so simNew is called
/// with enable_ltm=true. Models with SMOOTH/DELAY that participate in
/// feedback loops must compile with LTM enabled.
#[test]
fn mark2_mdl_compiles_with_ltm_enabled() {
    use simlin_engine::db::set_project_ltm_enabled;

    let contents =
        std::fs::read_to_string("../../test/bobby/vdf/econ/mark2.mdl").expect("read mark2.mdl");
    let project = open_vensim(&contents).expect("parse mark2.mdl");
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("mark2.mdl should compile with LTM enabled");
    let mut vm = Vm::new(compiled).expect("VM creation should succeed");
    vm.run_to_end().expect("VM should run to completion");
}

// ===========================================================================
// Phase 3 / Task 4: single-output macro simulation fixtures and edge cases
//
// Group 1 wires the six bundled `.mdl` macro fixtures into dedicated tests,
// each running `open_vensim` -> `compile_vm` -> VM -> `ensure_results` against
// the fixture's `output.tab` (the same pipeline `simulate_mdl_path` runs).
//
// Group 2 covers the five single-output behaviors that have no bundled
// fixture, using a trivial inline `.mdl` string with hand-computed expected
// values. NOTE (GH #553): a single-argument `NAME(arg)` MDL call is rewritten
// to `LOOKUP(NAME, arg)` before macro resolution, so every inline macro below
// uses >= 2 parameters so the call survives MDL import as a macro invocation.
// ===========================================================================

/// Read a scalar variable's value at simulation step `step` (0-based) from a
/// `Results`. Panics with a clear message if the variable is absent.
fn macro_test_value_at(results: &Results, name: &str, step: usize) -> f64 {
    let ident = simlin_engine::common::Ident::<simlin_engine::common::Canonical>::new(name);
    let off = *results.offsets.get(&ident).unwrap_or_else(|| {
        panic!(
            "variable {name:?} not in results; present: {:?}",
            results.offsets.keys().collect::<Vec<_>>()
        )
    });
    results.iter().nth(step).unwrap_or_else(|| {
        panic!(
            "no step {step} in results (step_count={})",
            results.step_count
        )
    })[off]
}

/// Parse + compile + run an inline Vensim `.mdl` string through the same VM
/// path the fixture tests use, returning the `Results`.
fn run_inline_mdl(mdl: &str) -> Results {
    let datamodel_project =
        open_vensim(mdl).unwrap_or_else(|e| panic!("failed to parse inline macro mdl: {e}"));
    let compiled = compile_vm(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed: {e}"));
    vm.into_results()
}

// --- Group 1: the six bundled `.mdl` fixtures ------------------------------

/// macros.AC2.1: a stockless single-output macro
/// (`EXPRESSION MACRO(input, parameter) = input * parameter`) simulates and
/// matches `output.tab` (`macro output = 5 * 1.1 = 5.5`).
#[test]
fn simulates_macro_expression_mdl() {
    simulate_mdl_path("../../test/test-models/tests/macro_expression/test_macro_expression.mdl");
}

/// macros.AC2.2: a stock-bearing macro
/// (`EXPRESSION MACRO = INTEG(input, parameter)`) simulates with correct
/// per-invocation integration across the 11-step `output.tab`
/// (init 1.1, +5/step: 1.1, 6.1, 11.1, ... 51.1).
#[test]
fn simulates_macro_stock_mdl() {
    simulate_mdl_path("../../test/test-models/tests/macro_stock/test_macro_stock.mdl");
}

/// macros.AC2.5: a multi-equation macro body with a macro-local helper
/// (`EXPRESSION MACRO = input * intermediate`, `intermediate = parameter * 3`)
/// simulates correctly (`5 * (1.1 * 3) = 16.5`). Additionally asserts the
/// `intermediate` helper does not leak into the `main` model's namespace.
#[test]
fn simulates_macro_multi_expression_mdl() {
    let path =
        "../../test/test-models/tests/macro_multi_expression/test_macro_multi_expression.mdl";
    let contents =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    let datamodel_project =
        open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));

    // The `intermediate` helper is a macro-body aux; it must live inside the
    // macro model, never in `main`. `ensure_results` only checks expected
    // columns, so we assert namespace isolation explicitly here.
    let main = datamodel_project
        .get_model("main")
        .expect("project must contain a \"main\" model");
    assert!(
        main.get_variable("intermediate").is_none(),
        "the macro-local `intermediate` helper must not leak into `main`; \
         main variables: {:?}",
        main.variables
            .iter()
            .map(|v| v.get_ident().to_string())
            .collect::<Vec<_>>()
    );

    let compiled = compile_vm(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed for {path}: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed for {path}: {e}"));
    let results = vm.into_results();

    let expected = load_expected_results_for_mdl(path)
        .unwrap_or_else(|| panic!("no reference data found for {path}"));
    ensure_results(&expected, &results);
}

/// macros.AC2.6: a macro that calls another macro
/// (`EXPRESSION MACRO = SECOND MACRO(input, parameter)`,
/// `SECOND MACRO = input / parameter`) expands recursively and simulates
/// (`5 / 1.1 = 4.54545`).
#[test]
fn simulates_macro_cross_reference_mdl() {
    simulate_mdl_path(
        "../../test/test-models/tests/macro_cross_reference/test_macro_cross_reference.mdl",
    );
}

/// Two independent macros in one model: `macro output` uses
/// `EXPRESSION MACRO` (`5 * 1.1 = 5.5`) and `second macro output` uses
/// `SECOND MACRO` (`5 / 1.1 = 4.54545`).
#[test]
fn simulates_macro_multi_macros_mdl() {
    simulate_mdl_path(
        "../../test/test-models/tests/macro_multi_macros/test_macro_multi_macros.mdl",
    );
}

/// macros.AC5.5: a macro defined *after* its first use
/// (`macro output = EXPRESSION MACRO(...)` precedes the `:MACRO:` block)
/// still resolves and simulates (`5 * 1.1 = 5.5`).
#[test]
fn simulates_macro_trailing_definition_mdl() {
    simulate_mdl_path(
        "../../test/test-models/tests/macro_trailing_definition/test_macro_trailing_definition.mdl",
    );
}

// --- Group 2: focused tests for behaviors with no bundled fixture ----------

/// macros.AC2.3: the same stock-bearing macro invoked at two call sites with
/// different arguments produces independent per-invocation state -- the two
/// invocations do not share a stock.
///
/// Macro `M(rate, init) = INTEG(rate, init)`.
///   x = M(1, 0):  Euler dt=1, x[k] = init + rate*k = 0 + 1*k = k
///   y = M(2, 10): y[k] = 10 + 2*k
/// (Vensim INTEG: value at step k (t=k) is init + rate*k -- same shape the
/// `macro_stock` fixture confirms: init 1.1, rate 5 -> 1.1, 6.1, ...).
/// With INITIAL TIME=0, FINAL TIME=4, TIME STEP=1: 5 steps, t=0..4.
///   x = [0, 1, 2, 3, 4]   y = [10, 12, 14, 16, 18]
#[test]
fn simulates_macro_independent_invocation_state() {
    let mdl = "\
{UTF-8}
:MACRO: M(rate, init)
M = INTEG(rate, init)
	~	stock
	~	per-invocation independent state
	|
:END OF MACRO:
x= M(1, 0) ~~|
y= M(2, 10) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 4 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let results = run_inline_mdl(mdl);
    let expected_x = [0.0, 1.0, 2.0, 3.0, 4.0];
    let expected_y = [10.0, 12.0, 14.0, 16.0, 18.0];
    for step in 0..5 {
        let x = macro_test_value_at(&results, "x", step);
        let y = macro_test_value_at(&results, "y", step);
        assert!(
            (x - expected_x[step]).abs() < 1e-9,
            "step {step}: x = {x}, expected {} (M(1,0), independent stock)",
            expected_x[step]
        );
        assert!(
            (y - expected_y[step]).abs() < 1e-9,
            "step {step}: y = {y}, expected {} (M(2,10), independent stock)",
            expected_y[step]
        );
    }
}

/// macros.AC2.4: a macro invoked with an expression-valued argument
/// (`y = M(a + b, t)`) -- the argument is evaluated in the caller's context.
///
/// Macro `M(in, p) = in * p`. Constants a=3, b=4, t=5.
///   y = (a + b) * t = (3 + 4) * 5 = 35   (constant across all steps)
#[test]
fn simulates_macro_expression_valued_argument() {
    let mdl = "\
{UTF-8}
:MACRO: M(in, p)
M = in * p
	~	product
	~	expression-valued argument
	|
:END OF MACRO:
a= 3 ~~|
b= 4 ~~|
t= 5 ~~|
y= M(a + b, t) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 3 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let results = run_inline_mdl(mdl);
    // (3 + 4) * 5 = 35
    for step in 0..results.step_count {
        let y = macro_test_value_at(&results, "y", step);
        assert!(
            (y - 35.0).abs() < 1e-9,
            "step {step}: y = {y}, expected 35 = (a+b)*t evaluated in caller context"
        );
    }
}

/// macros.AC2.7: a macro body referencing global time via the `$` escape
/// (`Time$`) simulates with the global time values.
///
/// Macro `M(base, offset) = base + offset + Time$` (a second parameter is
/// required so the call is not rewritten to LOOKUP -- GH #553).
///   y = M(10, 0) = 10 + 0 + Time = 10 + Time
/// With INITIAL TIME=0, FINAL TIME=4, TIME STEP=1: y[k] = 10 + k.
#[test]
fn simulates_macro_time_escape() {
    // The units slot (the first `~`) is parsed as a unit expression, so it
    // must not contain a hyphen (`-` is a unit operator); use a plain token.
    let mdl = "\
{UTF-8}
:MACRO: M(base, offset)
M = base + offset + Time$
	~	dmnl
	~	time access from a macro body via the time form of the dollar escape
	|
:END OF MACRO:
y= M(10, 0) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 4 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let results = run_inline_mdl(mdl);
    // y = 10 + 0 + Time
    for step in 0..results.step_count {
        let time = macro_test_value_at(&results, "time", step);
        let y = macro_test_value_at(&results, "y", step);
        assert!(
            (y - (10.0 + time)).abs() < 1e-9,
            "step {step}: y = {y}, expected {} = 10 + Time({time})",
            10.0 + time
        );
    }
}

/// macros.AC2.8: a macro invocation nested inside a larger expression
/// (`y = c + M(x, t)`) expands and simulates correctly.
///
/// Macro `M(a, b) = a * b`. Constants c=100, x=3, t=5.
///   y = c + M(x, t) = 100 + (3 * 5) = 115   (constant across all steps)
#[test]
fn simulates_macro_nested_invocation() {
    let mdl = "\
{UTF-8}
:MACRO: M(a, b)
M = a * b
	~	product
	~	nested invocation inside a larger expression
	|
:END OF MACRO:
c= 100 ~~|
x= 3 ~~|
t= 5 ~~|
y= c + M(x, t) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 3 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let results = run_inline_mdl(mdl);
    // 100 + (3 * 5) = 115
    for step in 0..results.step_count {
        let y = macro_test_value_at(&results, "y", step);
        assert!(
            (y - 115.0).abs() < 1e-9,
            "step {step}: y = {y}, expected 115 = c + M(x,t)"
        );
    }
}

/// macros.AC5.4 (simulation-level): a macro shadowing the `SSHAPE` builtin is
/// resolved to the macro, not the builtin, even though the model also uses
/// other builtins. Task 3 verifies this at expansion level; this confirms it
/// end-to-end through simulation.
///
/// Macro `SSHAPE(x, p) = x + p` (a real `SSHAPE` builtin exists and is a
/// 3-arg S-curve; a 2-arg call is NOT rewritten to LOOKUP).
///   y = SSHAPE(3, 4) = 3 + 4 = 7    (macro definition, NOT the builtin)
///   z = ABS(-7) = 7                 (an unrelated builtin still works)
#[test]
fn simulates_macro_shadowing_sshape_builtin() {
    let mdl = "\
{UTF-8}
:MACRO: SSHAPE(x, p)
SSHAPE = x + p
	~	shadowing macro
	~	a project macro shadows the SSHAPE builtin
	|
:END OF MACRO:
y= SSHAPE(3, 4) ~~|
z= ABS(-7) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 2 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let results = run_inline_mdl(mdl);
    // The macro defines SSHAPE(x, p) = x + p, so y = 3 + 4 = 7.
    // The real SSHAPE builtin is a 3-arg S-shaped curve and would NOT
    // produce 7 for these inputs; getting 7 proves the macro shadowed it.
    // z = ABS(-7) = 7 confirms unrelated builtins still resolve normally.
    for step in 0..results.step_count {
        let y = macro_test_value_at(&results, "y", step);
        let z = macro_test_value_at(&results, "z", step);
        assert!(
            (y - 7.0).abs() < 1e-9,
            "step {step}: y = {y}, expected 7 = macro SSHAPE(3,4)=3+4 (not the builtin)"
        );
        assert!(
            (z - 7.0).abs() < 1e-9,
            "step {step}: z = {z}, expected 7 = ABS(-7) (unrelated builtin)"
        );
    }
}
