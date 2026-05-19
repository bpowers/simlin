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

use test_helpers::{ensure_results, ensure_results_excluding};

const OUTPUT_FILES: &[(&str, u8)] = &[("output.csv", b','), ("output.tab", b'\t')];

static TEST_MODELS: &[&str] = &[
    // failing testcases (various reasons)
    // "test/test-models/tests/arguments/test_arguments.xmile",
    // "test/test-models/tests/delay_parentheses/test_delay_parentheses.xmile",
    // "test/test-models/tests/delay_pipeline/test_pipeline_delays.xmile",
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
    // Macro fixtures: each `<macro>` element imports as a macro-marked model
    // (Phase 5 Task 1 reader), the invocation expands and simulates against
    // `output.tab` (Phase 3), and `simulate_path_with` re-serializes the
    // project to XMILE and asserts a byte-stable round-trip (Phase 5 Task 2
    // writer). `macro_multi_macros` exercises two `<macro>` elements;
    // `macro_stock` a stock-bearing macro body. (The `.stmx` variants and the
    // `macro_cross_reference`/`macro_trailing_definition` dirs have no
    // `<macro>` element, so they are not wired here.)
    "test/test-models/tests/macro_expression/test_macro_expression.xmile",
    "test/test-models/tests/macro_multi_expression/test_macro_multi_expression.xmile",
    "test/test-models/tests/macro_multi_macros/test_macro_multi_macros.xmile",
    "test/test-models/tests/macro_stock/test_macro_stock.xmile",
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
    simulate_path_with_excluding(xmile_path, compile, &[]);
}

/// Like [`simulate_path_with`], but carves a set of base-variable names out
/// of the comparison *and* the compiled model. Each excluded variable is
/// (a) removed from every datamodel model before compilation -- so a single
/// not-yet-supported variable does not block the whole model from
/// compiling -- and (b) skipped in every comparison path (VM, protobuf
/// round-trip, XMILE round-trip). Every *other* variable stays a hard
/// genuine-Vensim equality gate. Used to keep `vector.xmile`'s ELM MAP
/// base/full-source variables (`c`/`f`/`g`) as hard gates while excluding
/// only `y` (GitHub #578), rather than weakening the whole comparison.
fn simulate_path_with_excluding(xmile_path: &str, compile: CompileFn, excluded: &[&str]) {
    eprintln!("model: {xmile_path}");

    let datamodel_project = {
        let f = File::open(xmile_path).unwrap();
        let mut f = BufReader::new(f);

        let datamodel_project = xmile::project_from_reader(&mut f);
        if let Err(ref err) = datamodel_project {
            eprintln!("model '{xmile_path}' error: {err}");
        }
        let mut datamodel_project = datamodel_project.unwrap();
        // Drop excluded variables from every model. An excluded variable is
        // one we intentionally do not gate on yet (tracked separately); if
        // it fails to compile it would otherwise abort the whole project
        // and prevent gating the variables we DO support.
        for model in &mut datamodel_project.models {
            model
                .variables
                .retain(|v| !excluded.contains(&v.get_ident()));
        }
        datamodel_project
    };

    // simulate the model using our bytecode VM
    let results = {
        let compiled_sim = compile(&datamodel_project);
        let mut vm = Vm::new(compiled_sim).unwrap();
        vm.run_to_end().unwrap();
        vm.into_results()
    };

    let expected = load_expected_results(xmile_path).unwrap();
    ensure_results_excluding(&expected, &results, excluded);

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
    ensure_results_excluding(&expected, &results_proto, excluded);

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
    ensure_results_excluding(&expected, &results_xmile, excluded);

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
    // VECTOR ELM MAP now matches genuine Vensim (per-element base + full
    // source array, out-of-range -> :NA:, no modulo). vector.xmile is
    // exercised through all three comparison paths by the dedicated
    // `simulates_vector_xmile_genuine` test below, NOT here: that test keeps
    // c/f/g (the ELM MAP base/full-source variables) and every other
    // variable as hard genuine-Vensim gates against
    // test/sdeverywhere/models/vector/vector.dat, narrowing the comparison
    // to exclude only two pre-existing, separately-tracked, out-of-scope
    // variables -- `y` (GitHub #578: scalar-source/expression-offset ELM MAP
    // does not compile) and `p` (GitHub #576: dormant/unverified 2-D VECTOR
    // SORT ORDER fixture data; a different builtin, unchanged here). This
    // list runs an unconditional full comparison, which cannot carve out
    // those variables; the narrowed gate lives in its own test instead of
    // weakening every model's comparison.
    // "test/sdeverywhere/models/vector/vector.xmile",  // -> simulates_vector_xmile_genuine
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

/// Genuine-Vensim regression gate for VECTOR ELM MAP cross-dimension
/// resolution (element-cycle-resolution.AC6.3 / AC6.2). `vector.xmile` is
/// compared against the real-Vensim `vector.dat` through all three
/// `ensure_results` paths (VM, protobuf round-trip, XMILE round-trip).
///
/// Every variable is a hard genuine-Vensim equality gate. In particular the
/// ELM MAP base/full-source variables this phase fixes must match
/// `vector.dat` exactly: `c[A1..A3] = 10 + VECTOR ELM MAP(b[B1], a[DimA])`
/// is `11, 12, 12`; `f[DimA,DimB] = VECTOR ELM MAP(d[DimA,B1], a[DimA])` is
/// `1, 5, 6` (broadcast across DimB); and
/// `g[DimA,DimB] = VECTOR ELM MAP(d[DimA,B1], e[DimA,DimB])` is
/// `1, 4, 5, 2, 3, 6`. So do every non-ELM-MAP variable the model also
/// exercises (1-D VSO `l`/`m`, VECTOR SELECT `q`/`r`/`s`, reducers
/// `u`/`v`/`w`, and the rest).
///
/// Exactly two variables are carved out, both pre-existing and
/// separately-tracked gaps unrelated to the ELM MAP base/full-source fix
/// (per the phase file's "prefer full inclusion; narrow only with a tracked
/// issue" guidance). First, `y[DimA] = VECTOR ELM MAP(x[three], (DimA-1))`
/// (GitHub #578): a scalar source plus an arithmetic (expression) offset
/// from which ELM MAP cannot yet infer a result shape, so `y` does not
/// compile at all -- a compiler shape-inference gap, NOT the base/stride
/// numeric bug fixed here; its genuine value `y[A1]=3,y[A2]=4,y[A3]=5` is
/// in `vector.dat`, and closing #578 lets `y` rejoin this gate. Second,
/// `p[DimA,DimB] = VECTOR SORT ORDER(o[DimA,DimB], ASCENDING)` (GitHub
/// #576): a genuinely 2-D VSO whose `vector.dat` `p` block is internally
/// inconsistent / encodes an sdeverywhere per-row semantic, with
/// genuine-Vensim multi-dimensional VSO semantics unverified by any live
/// fixture -- a different builtin (VSO, unchanged by this phase) out of
/// Phase 5's ELM MAP scope.
///
/// Excluded variables are dropped from the compiled model (so #578's
/// non-compiling `y` cannot abort the project) and skipped in every
/// comparison; the genuine gate on `c`/`f`/`g` (and all other variables)
/// is NOT weakened.
#[test]
fn simulates_vector_xmile_genuine() {
    simulate_path_with_excluding(
        "../../test/sdeverywhere/models/vector/vector.xmile",
        compile_vm,
        // y: GitHub #578 (scalar-source/expression-offset ELM MAP compile
        // gap). p: GitHub #576 (dormant/unverified 2-D VSO fixture data).
        // Both pre-existing and out of Phase 5's ELM MAP scope.
        &["y", "p"],
    );
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

// FULL end-to-end C-LEARN simulation against `Ref.vdf`. Still `#[ignore]`d,
// but the blocker is NO LONGER macro expansion.
//
// After Phases 1-6 + GH #554 (the false `init -> init` macro-registry
// recursion fix), C-LEARN's four macros (SAMPLE UNTIL, SSHAPE, RAMP FROM TO,
// INIT) parse, register, and expand with ZERO macro-attributable
// diagnostics -- this is asserted by `corpus_clearn_macros_import`
// (macros.AC6.2) and the three `simulates_macro_clearn_*` focused fixtures
// (macros.AC6.3) exercise each invoked macro's defined behavior. The macro
// work is DONE.
//
// What remains are C-LEARN's *non-macro* blockers, which the design
// explicitly scopes OUT of the macro work ("tracked separately"):
// `compile_vm` fails with `NotSimulatable: model 'main' has circular
// dependencies` before the VM is even constructed. The collected
// diagnostics (see `corpus_clearn_macros_import`'s compile step) show the
// concrete non-macro causes, all on the `main` model:
//   * a model-logic `CircularDependency` on
//     `main.previous_emissions_intensity_vs_refyr` (NOT the project-level
//     macro-registry recursion #554 fixed -- this one is attributed to a
//     real `main` variable);
//   * `MismatchedDimensions` on `c_in_mixed_layer`,
//     `heat_in_atmosphere_and_upper_ocean`, `c_in_deep_ocean_net_flow`,
//     `heat_in_deep_ocean_net_flow` (subscript/dimension issues);
//   * `UnknownDependency` on `emissions_with_cumulative_constraints` and a
//     non-time `$` reference surfacing as a `DoesNotExist` on
//     `"goal_1.5_for_temperature"` (Phase 3's documented-limitation:
//     deprioritized, surfaces as an ordinary unresolved reference -- NOT a
//     macro error);
//   * plus unit-inference warnings (non-fatal).
// These are tracked separately per the design (the parent should file /
// confirm tracking issues for the C-LEARN non-macro blockers and full
// end-to-end C-LEARN simulation -- they are out of scope for the macro
// work, which is complete). Keep `#[ignore]`d until they are resolved.
// Run with: cargo test --release -- --ignored simulates_clearn
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

/// Issue #559 -- the C-LEARN `"Goal 1.5 for Temperature"` shape.
///
/// A Vensim quoted identifier containing a literal period (`"a.b"`,
/// `"goal 1.5"`) used to make the whole `main` model `NotSimulatable`:
/// `canonicalize()` was non-idempotent for quoted-period idents (first
/// pass kept a raw `.`, which `is_canonical()` rejects, so a re-canonical
/// pass mis-converted the literal period into the `·` module-hierarchy
/// separator -> the variable's own identity split into a phantom submodule
/// -> `DoesNotExist`). It tripped even for a *dead* constant referenced by
/// nobody, because it is the variable's own fragment identity that fails.
/// Run through the same incremental `compile_vm` path `simulates_clearn`
/// uses. Two shapes:
///   A. a quoted-period constant *referenced* by a live var, proving its
///      identity resolves AND is usable in equations (`y = "a.b" + 1`);
///   B. a *dead* quoted-period constant plus an unrelated live `z`,
///      proving such a constant no longer poisons compilation and the fix
///      changes no live output.
#[test]
fn simulates_quoted_period_ident() {
    // A: quoted-period constant referenced by a live variable.
    let mdl_a = "\
{UTF-8}
\"a.b\" = 5 ~~|
y = \"a.b\" + 1 ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 2 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let results = run_inline_mdl(mdl_a);
    for step in 0..3 {
        let y = macro_test_value_at(&results, "y", step);
        assert!(
            (y - 6.0).abs() < 1e-9,
            "step {step}: y = {y}, expected 6 (= \"a.b\"(5) + 1); the \
             quoted-period ident must resolve and be usable"
        );
    }

    // B: a DEAD quoted-period constant (the exact C-LEARN
    // `"Goal 1.5 for Temperature"` shape) plus an unrelated live `z`.
    // Before the fix this alone made the model NotSimulatable.
    let mdl_b = "\
{UTF-8}
\"goal 1.5\" = 1.5 ~~|
z = 1 ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 2 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let results = run_inline_mdl(mdl_b);
    for step in 0..3 {
        let z = macro_test_value_at(&results, "z", step);
        assert!(
            (z - 1.0).abs() < 1e-9,
            "step {step}: z = {z}, expected 1; a dead quoted-period \
             constant must not make the model NotSimulatable"
        );
    }
}

/// Read a `[t1]/[t2]/[t3]`-style element series out of a `Results`.
fn element_series(results: &Results, name: &str) -> Vec<f64> {
    let id = simlin_engine::common::Ident::<simlin_engine::common::Canonical>::new(name);
    let off = *results.offsets.get(&id).unwrap_or_else(|| {
        panic!(
            "{name:?} absent; have {:?}",
            results.offsets.keys().collect::<Vec<_>>()
        )
    });
    (0..results.step_count)
        .map(|s| results.iter().nth(s).unwrap()[off])
        .collect()
}

/// Compile an inline/loaded Vensim `.mdl` through the incremental path and
/// return `(compile_is_err, diagnostics)` WITHOUT panicking -- for fixtures
/// that are intentionally NOT simulatable (cycle/recurrence repros), where
/// we assert the *diagnostic code*, not a simulation result.
fn compile_diags(mdl: &str) -> (bool, Vec<simlin_engine::db::Diagnostic>) {
    let dm = open_vensim(mdl).unwrap_or_else(|e| panic!("failed to parse mdl: {e}"));
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &dm, None);
    let is_err = compile_project_incremental(&db, sync.project, "main").is_err();
    (is_err, collect_project_diagnostics(&dm))
}

fn diag_code(d: &simlin_engine::db::Diagnostic) -> Option<simlin_engine::common::ErrorCode> {
    use simlin_engine::db::DiagnosticError;
    match &d.error {
        DiagnosticError::Equation(e) => Some(e.code),
        DiagnosticError::Model(e) => Some(e.code),
        _ => None,
    }
}

/// Issue #559 + element-level cycle resolution (Phase 1) -- the C-LEARN
/// `Emissions with cumulative constraints[COP,tNext] = ... ecc[COP,tPrev]`
/// self-recurrence shape, minimised to a single self-recurrent variable
/// with NO cross-variable cycle (`test/sdeverywhere/models/self_recurrence`):
/// `ecc[t1]=1; ecc[tNext]=ecc[tPrev]+1` over the subrange `t1..t3`.
///
/// Two intents, both verified here:
///
/// (#559 name-resolution guard) The native MDL converter
/// (`xmile_compat.rs::format_var_ctx`) rewrote a self-reference
/// (`name == ctx.lhs_var_canonical`) to the literal token `self`; for a
/// *subscripted* self-reference it emitted `self[..]`. The engine's
/// `builtins_visitor` Subscript arm never resolves `self`, so the literal
/// token leaked into dependency analysis as an undefined name ->
/// `UnknownDependency`. The fix emits the real canonical LHS name instead,
/// making the self-reference an ordinary reference; the `self` token must
/// never leak as `UnknownDependency`/`DoesNotExist` again.
///
/// (element-cycle-resolution.AC1.1/AC1.2 target behavior) Once `self`
/// resolves, `ecc[tNext]=ecc[tPrev]+1` is a whole-variable self-edge whose
/// *induced element graph* (`ecc[t2]<-ecc[t1]`, `ecc[t3]<-ecc[t2]`) is
/// acyclic and well-founded. The element-level cycle refinement resolves
/// it: the model compiles via the incremental path with NO
/// `CircularDependency` and simulates to the deterministic staggered
/// series `ecc[t1]=1, ecc[t2]=2, ecc[t3]=3` (constant across both saved
/// steps -- the recurrence is over the subrange, not over time). FINAL
/// TIME=1, TIME STEP=1 => 2 saved steps. `self_recurrence/` ships no
/// `.dat`, so the series is asserted in-test via `element_series`.
#[test]
fn self_recurrence_resolves_and_no_self_token_leak() {
    use simlin_engine::common::ErrorCode;

    let mdl = std::fs::read_to_string(
        "../../test/sdeverywhere/models/self_recurrence/self_recurrence.mdl",
    )
    .expect("self_recurrence.mdl fixture must exist");
    let (compile_err, diags) = compile_diags(&mdl);

    // `self_recurrence` has exactly one non-control variable (`ecc`) and
    // self-contained dims, so the ONLY possible unknown is the leaked
    // `self` token.
    let self_leak = diags.iter().any(|d| {
        matches!(
            diag_code(d),
            Some(ErrorCode::UnknownDependency) | Some(ErrorCode::DoesNotExist)
        )
    });
    let has_circular = diags
        .iter()
        .any(|d| diag_code(d) == Some(ErrorCode::CircularDependency));

    // (AC1.1) The induced element graph is acyclic, so the model now
    // compiles via the incremental path with no CircularDependency (this
    // inverts the pre-resolution assertions: it used to be NotSimulatable
    // with a whole-variable self-edge CircularDependency).
    assert!(
        !compile_err,
        "self_recurrence.mdl must now compile via the incremental path: \
         its single-variable self-recurrence has an acyclic induced \
         element graph and is resolved by the element-cycle refinement. \
         Diagnostics: {diags:#?}"
    );
    assert!(
        !has_circular,
        "the single-variable self-recurrence must NOT report \
         CircularDependency once the element-cycle refinement resolves it \
         (its induced element graph is acyclic). Diagnostics: {diags:#?}"
    );
    // (#559 guard, preserved) The `self` token resolves to the enclosing
    // var, so it never leaks as an unknown dependency. (Before the #559
    // fix this assertion failed: `self[tPrev]` leaked as
    // UnknownDependency because the converter emitted the literal `self`
    // and the builtins_visitor Subscript arm never resolved it.)
    assert!(
        !self_leak,
        "the literal `self` token still leaks as \
         UnknownDependency/DoesNotExist. The converter must emit the real \
         canonical LHS name, not `self`. Diagnostics: {diags:#?}"
    );

    // (AC1.2) It simulates to the well-founded staggered series
    // ecc[t1]=1, ecc[t2]=2, ecc[t3]=3 -- constant across both saved steps
    // (the recurrence is over the subrange, not over time).
    let r = run_inline_mdl(&mdl);
    assert_eq!(r.step_count, 2);
    assert_eq!(element_series(&r, "ecc[t1]"), vec![1.0, 1.0]);
    assert_eq!(element_series(&r, "ecc[t2]"), vec![2.0, 2.0]);
    assert_eq!(element_series(&r, "ecc[t3]"), vec![3.0, 3.0]);
}

/// Genuine-cycle guards: the self-reference fix must NOT weaken real
/// cycle detection. These two models have NO well-founded staggered
/// recurrence -- they are genuine cycles and MUST stay rejected (a
/// CLAUDE.md hard rule).
#[test]
fn genuine_cycles_still_rejected() {
    use simlin_engine::common::ErrorCode;

    // (1) Scalar 2-cycle `a=b+1; b=a+1`: a true algebraic loop, no `self`
    // token involved at all -> `CircularDependency`, stable before and
    // after the fix.
    let two_cycle = "\
{UTF-8}
a = b + 1 ~~|
b = a + 1 ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 1 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let (err1, diags1) = compile_diags(two_cycle);
    assert!(err1, "scalar 2-cycle a=b+1;b=a+1 must be NotSimulatable");
    assert!(
        diags1
            .iter()
            .any(|d| diag_code(d) == Some(ErrorCode::CircularDependency)),
        "scalar 2-cycle must report CircularDependency (genuine cycle \
         detection must not be weakened). Diagnostics: {diags1:#?}"
    );
    assert!(
        !diags1.iter().any(|d| matches!(
            diag_code(d),
            Some(ErrorCode::UnknownDependency) | Some(ErrorCode::DoesNotExist)
        )),
        "scalar 2-cycle has no undefined names; only CircularDependency \
         is expected. Diagnostics: {diags1:#?}"
    );

    // (2) Genuine SAME-element self `x[dimA]=x[dimA]+1` (NOT a shifted
    // subrange): a real same-element self-cycle that MUST stay rejected.
    // element-cycle-resolution.AC4.2: element-level cycle resolution
    // resolves the `self` token to the real name (`x`), so every element
    // reads *itself* (`x[a1]<-x[a1]`, `x[a2]<-x[a2]`). The induced element
    // graph therefore has an element self-loop -- it is element-cyclic, a
    // genuine cycle -- so the engine MUST report `CircularDependency`
    // SPECIFICALLY. This is no longer an `UnknownDependency` leak: that
    // only happened before the #559 fix, when the literal `self` token
    // leaked as an undefined name. Pin `CircularDependency` exactly (the
    // assertion below also forbids any `UnknownDependency`/`DoesNotExist`,
    // which would now be a name-resolution regression, not the expected
    // verdict).
    let same_elem = "\
{UTF-8}
dimA: a1, a2 ~~|
x[dimA] = x[dimA] + 1 ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 1 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let (err2, diags2) = compile_diags(same_elem);
    assert!(
        err2,
        "genuine same-element self x[dimA]=x[dimA]+1 must stay \
         NotSimulatable (CLAUDE.md hard rule -- a real cycle)"
    );
    assert!(
        diags2
            .iter()
            .any(|d| diag_code(d) == Some(ErrorCode::CircularDependency)),
        "genuine same-element self x[dimA]=x[dimA]+1 must report \
         CircularDependency SPECIFICALLY (element-cycle-resolution.AC4.2): \
         every element reads itself, so the induced element graph has an \
         element self-loop and the element-cycle refinement keeps the \
         conservative CircularDependency. Diagnostics: {diags2:#?}"
    );
    assert!(
        !diags2.iter().any(|d| matches!(
            diag_code(d),
            Some(ErrorCode::UnknownDependency) | Some(ErrorCode::DoesNotExist)
        )),
        "the `self` token must resolve to the real name `x`; a leaked \
         UnknownDependency/DoesNotExist here is a #559 name-resolution \
         regression, not the expected same-element-self-cycle verdict. \
         Diagnostics: {diags2:#?}"
    );
}

/// Regression guard for the one self-context that must keep working: a
/// self-reference INSIDE `PREVIOUS()`. The #559 fix stops emitting the
/// literal `self` in `xmile_compat.rs::format_var_ctx`; this proves it
/// does not perturb the self-in-PREVIOUS path.
///
/// Primary case is the *shifted subrange* form
/// `x[t1]=1; x[tNext]=PREVIOUS(x[tPrev],0)`. Before the fix this failed
/// to compile -- the synthetic PREVIOUS-helper fragment was built around
/// the unresolved `self[tPrev]`; after the fix the real name `x[tPrev]`
/// resolves, the helper compiles, and it simulates the well-defined
/// PREVIOUS-shift recurrence. The scalar `x=PREVIOUS(x,0)` sub-case is
/// the simplest stable form.
#[test]
fn previous_self_reference_still_resolves() {
    // Sub-case A: scalar self-in-PREVIOUS (the simplest form).
    let scalar = "\
{UTF-8}
x = PREVIOUS(x, 0) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 2 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let (err_s, diags_s) = compile_diags(scalar);
    assert!(
        !err_s,
        "scalar x=PREVIOUS(x,0) must stay simulatable after the fix. \
         Diagnostics: {diags_s:#?}"
    );
    assert_eq!(run_inline_mdl(scalar).step_count, 3);

    // Sub-case B (primary): shifted-subrange self-in-PREVIOUS. Must
    // compile AND simulate the exact PREVIOUS-shift recurrence after the
    // fix.
    let shifted = "\
{UTF-8}
Target: (t1-t3)
\t~
\t~\t\t|

tNext: (t2-t3) -> tPrev
\t~
\t~\t\t|

tPrev: (t1-t2) -> tNext
\t~
\t~\t\t|
x[t1] = 1 ~~|

x[tNext] = PREVIOUS(x[tPrev], 0) ~~|

INITIAL TIME = 0 ~~|
FINAL TIME = 2 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let (err_b, diags_b) = compile_diags(shifted);
    assert!(
        !err_b,
        "shifted-subrange PREVIOUS self-ref must COMPILE after the fix \
         (resolving `self[tPrev]` -> `x[tPrev]` fixes the synthetic \
         PREVIOUS-helper that failed before). Diagnostics: {diags_b:#?}"
    );
    let r = run_inline_mdl(shifted);
    assert_eq!(r.step_count, 3);
    // x[t1]=1 (base, constant); x[t2]=PREVIOUS(x[t1],0); x[t3]=PREVIOUS(x[t2],0).
    // PREVIOUS = init at t0 else prior-DT value -> the deterministic
    // staggered-recurrence series:
    assert_eq!(element_series(&r, "x[t1]"), vec![1.0, 1.0, 1.0]);
    assert_eq!(element_series(&r, "x[t2]"), vec![0.0, 1.0, 1.0]);
    assert_eq!(element_series(&r, "x[t3]"), vec![0.0, 0.0, 1.0]);
}

/// `SAMPLE IF TRUE` expands in the converter to
/// `( IF cond THEN input ELSE PREVIOUS(SELF, init) )` via a hard-coded
/// literal `SELF` in the `"sample if true"` arm -- a *different* site
/// from the `format_var_ctx` self-reference rewrite the #559 fix changes.
/// The fix must NOT perturb this: a real `SAMPLE IF TRUE(...)` over a
/// subrange must still compile AND simulate correctly, proving the two
/// `self` sites are independent.
#[test]
fn sample_if_true_subrange_self_template_unaffected() {
    let mdl = "\
{UTF-8}
Target: (t1-t3)
\t~
\t~\t\t|

tNext: (t2-t3) -> tPrev
\t~
\t~\t\t|

tPrev: (t1-t2) -> tNext
\t~
\t~\t\t|
k = 5 ~~|

a = 7 ~~|

c = 9 ~~|

y[t1] = SAMPLE IF TRUE(Time <= k, a, c) ~~|

y[tNext] = SAMPLE IF TRUE(Time <= k, a, c) ~~|

INITIAL TIME = 0 ~~|
FINAL TIME = 2 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let (err, diags) = compile_diags(mdl);
    assert!(
        !err,
        "real SAMPLE IF TRUE over a subrange must compile after the fix \
         (the fix must not touch the hard-coded PREVIOUS(SELF,init) \
         template). Diagnostics: {diags:#?}"
    );
    let r = run_inline_mdl(mdl);
    assert_eq!(r.step_count, 3);
    // Time=0,1,2 are all <= k(5) -> condition always true -> output is the
    // sampled input a(7) at every element/step (the SELF/previous branch
    // is never taken; this exercises the template's structure intact).
    assert_eq!(element_series(&r, "y[t1]"), vec![7.0, 7.0, 7.0]);
    assert_eq!(element_series(&r, "y[t2]"), vec![7.0, 7.0, 7.0]);
    assert_eq!(element_series(&r, "y[t3]"), vec![7.0, 7.0, 7.0]);
}

/// element-cycle-resolution.AC2.1 (Phase 2 Task 7). `ref.mdl` is a
/// two-variable INTER-element recurrence: `ce[t1]=1;
/// ce[tNext]=ecc[tPrev]+1; ecc[t1]=ce[t1]+1; ecc[tNext]=ce[tNext]+1` over
/// the subrange `t1..t3`. Whole-variable `ce`<->`ecc` is a 2-cycle, but
/// the induced element graph
///   (ce,0)->(ecc,0); (ce,1)->(ecc,1); (ce,2)->(ecc,2);
///   (ecc,0)->(ce,1); (ecc,1)->(ce,2)
/// is acyclic. Before Subcomponent B (the GH #575 symbolic-ref
/// re-architecture: Task 4 verdict, Task 5 combined fragment, Task 5b
/// SCC-aware back-edge break, Task 6 injection) this multi-member SCC was
/// short-circuited to `CircularDependency` and the model did not compile.
/// Subcomponent B resolves the `{ce,ecc}` SCC, interleaves the members'
/// per-element segments in topological order, and injects one combined
/// fragment, so `ref.mdl` now compiles AND simulates to the hand-computed
/// per-element series shipped in the sibling `ref.dat`:
///   ce[t1]=1, ce[t2]=3, ce[t3]=5, ecc[t1]=2, ecc[t2]=4, ecc[t3]=6
/// (constant across both saved steps -- the recurrence is over the
/// subrange, not over time; FINAL TIME=1, TIME STEP=1 => 2 saved steps).
/// `simulate_mdl_path` loads `ref.dat` via `load_expected_results_for_mdl`
/// and compares with `ensure_results`, so this test IS AC2.1: it failed
/// before Subcomponent B (rejected as `CircularDependency`) and passes
/// now against `ref.dat`.
#[test]
fn ref_mdl_multi_variable_recurrence_simulates() {
    simulate_mdl_path("../../test/sdeverywhere/models/ref/ref.mdl");
}

/// element-cycle-resolution.AC2.2 (Phase 2 Task 8). `interleaved.mdl`:
/// `x=1; a[A1]=x; a[A2]=y; y=a[A1]; b[DimA]=a[DimA]` (`DimA: A1,A2`).
/// Whole-variable `a`<->`y` is a 2-cycle, but element-wise
/// `x -> a[A1] -> y -> a[A2]` is acyclic. Subcomponent B (GH #575)
/// resolves the multi-member `{a,y}` SCC and evaluates its members in
/// interleaved per-element order; before Subcomponent B this was rejected
/// as `CircularDependency` and the model did not compile. Every variable
/// resolves to `1.0`, so the sibling `interleaved.dat` is all `1.0`
/// across its 101 saved steps (INITIAL TIME=0, FINAL TIME=100, TIME
/// STEP=1 => 101 steps). `simulate_mdl_path` loads `interleaved.dat` via
/// `load_expected_results_for_mdl` and compares the VM output with
/// `ensure_results`, so this test IS AC2.2: it failed before
/// Subcomponent B and passes now.
#[test]
fn interleaved_mdl_element_interleave_simulates() {
    simulate_mdl_path("../../test/sdeverywhere/models/interleaved/interleaved.mdl");
}

/// element-cycle-resolution.AC2.4 (Phase 2 Task 9, the init-phase proof).
/// `init_recurrence.mdl` is a NEW fixture exercising the init-phase
/// combined-fragment path with a MULTI-member init SCC -- the first real
/// exercise of Task 6's synthetic-ident `SymbolicCompiledInitial` init
/// injection (`$⁚scc⁚init⁚{n}`) on a multi-member SCC. (Subcomponent A's
/// `init_recurrence_behind_stock_*` tests already cover the SINGLE-
/// variable init-recurrence-behind-a-stock case; AC2.4's combined-
/// fragment init path needs a MULTI-member init SCC.)
///
/// Shape: two arrayed stocks `cs[Target]` / `ecs[Target]` over the
/// subrange `t1..t3`, whose per-element INTEG **initial values** form a
/// `ref.mdl`-shaped inter-element recurrence ACROSS the two variables
/// (`cs[t1]=1; cs[tNext]=ecs[tPrev]+1; ecs[t1]=cs[t1]+1;
/// ecs[tNext]=cs[tNext]+1`), with a constant zero inflow `g[Target]=0`.
/// Each stock's dt-equation is its (acyclic) flow `g`, so the STOCK
/// BREAKS the dt chain -- there is NO dt cycle. The INIT relation,
/// however, has `cs`'s init referencing `ecs` and vice-versa: a
/// whole-variable init 2-cycle `{cs,ecs}` whose induced per-element INIT
/// graph is acyclic. So ONLY the init element graph exercises the
/// combined INIT fragment.
///
/// This test FIRST empirically confirms (per the AC2.4 mandate) that the
/// parsed fixture produces an init-ONLY MULTI-member element SCC -- the
/// production `model_dependency_graph` payload must carry exactly one
/// `ResolvedScc { phase: Initial }` with `>= 2` members (`{cs,ecs}`) and
/// `has_cycle == false` -- then simulates it via `simulate_mdl_path`
/// against the hand-computed `init_recurrence.dat`. The stocks integrate
/// a zero flow, so they hold their recurrence-computed initial values
/// constant across both saved steps (FINAL TIME=1, TIME STEP=1):
///   cs[t1]=1, cs[t2]=3, cs[t3]=5, ecs[t1]=2, ecs[t2]=4, ecs[t3]=6.
#[test]
fn init_recurrence_mdl_multi_member_init_scc_simulates() {
    use simlin_engine::common::Ident;
    use simlin_engine::db::{SccPhase, model_dependency_graph};
    use std::collections::BTreeSet;

    let path = "../../test/sdeverywhere/models/init_recurrence/init_recurrence.mdl";
    let contents =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("missing fixture {path}: {e}"));
    let dm = open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));

    // Empirical AC2.4 confirmation, tied to the REAL fixture: the parsed
    // MDL must yield an init-ONLY MULTI-member element SCC. If this
    // produced a 1-member or a dt-phase SCC the fixture would not
    // exercise the init combined-fragment path at all.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &dm, None);
    let sync = sync.to_sync_result();
    let model = sync.models["main"].source;
    let dep_graph = model_dependency_graph(&db, model, sync.project);

    assert!(
        !dep_graph.has_cycle,
        "init_recurrence.mdl: the element-acyclic MULTI-member init-only \
         recurrence behind stocks must NOT set has_cycle (resolved_sccs = \
         {:?})",
        dep_graph.resolved_sccs
    );
    assert_eq!(
        dep_graph.resolved_sccs.len(),
        1,
        "init_recurrence.mdl: exactly one ResolvedScc (the {{cs,ecs}} \
         init cluster) -- got {:?}",
        dep_graph.resolved_sccs
    );
    let scc = &dep_graph.resolved_sccs[0];
    assert_eq!(
        scc.phase,
        SccPhase::Initial,
        "init_recurrence.mdl: the resolved SCC MUST be phase == Initial \
         (a dt-phase SCC would mean the stock did not break the dt chain \
         and the fixture would not exercise the init combined-fragment \
         path) -- got {:?}",
        scc.phase
    );
    assert!(
        scc.members.len() >= 2,
        "init_recurrence.mdl: AC2.4 requires a MULTI-member init SCC (the \
         single-variable case is already covered by Subcomponent A); got \
         {} member(s): {:?}",
        scc.members.len(),
        scc.members
    );
    assert_eq!(
        scc.members,
        [Ident::new("cs"), Ident::new("ecs")]
            .into_iter()
            .collect::<BTreeSet<_>>(),
        "init_recurrence.mdl: the resolved init SCC's members must be \
         exactly {{cs,ecs}}"
    );

    // End-to-end: it compiles AND simulates to the hand-computed
    // init_recurrence.dat (the combined INIT fragment, injected as one
    // synthetic-ident SymbolicCompiledInitial, produces the correct
    // per-element initial values; the zero flow holds them constant).
    simulate_mdl_path(path);
}

/// element-cycle-resolution.AC3.1 (Phase 3 Task 3 -- the parent-sourcing
/// happy-path end-to-end proof). `helper_recurrence.mdl` is a NEW fixture:
/// a shift-mapped subrange recurrence over `Target: t1..t3` (the
/// `self_recurrence.mdl` `Target/tNext/tPrev` shape) whose per-element
/// recurrence body invokes a synthetic helper:
///
///   ecc[t1]     = 1
///   ecc[tNext]  = INITIAL(ecc[tPrev] * 2)
///
/// `INITIAL(<expr>)` (Vensim's spelling of XMILE `INIT`) over the shifted
/// subrange expands element-wise to `ecc[t2] = INITIAL(ecc[t1] * 2)` and
/// `ecc[t3] = INITIAL(ecc[t2] * 2)`. Because the `INITIAL` argument is an
/// *expression* (not a bare scalar slot), `builtins_visitor::make_temp_arg`
/// synthesizes a scalar helper aux per recurrence element -- the canonical
/// `$\u{205A}ecc\u{205A}0\u{205A}arg0\u{205A}t2` /
/// `$\u{205A}ecc\u{205A}0\u{205A}arg0\u{205A}t3` form (design deviation 2):
/// absent from `model.variables`, present in `model_implicit_var_info`
/// (its parent is `ecc`). Each helper's argument references `ecc`, and
/// `ecc`'s init references the helper (`ecc[tNext] = INITIAL($helper)`),
/// so the helpers land *inside* a MULTI-member init-phase recurrence SCC
/// `{ecc, $\u{205A}ecc\u{205A}0\u{205A}arg0\u{205A}t2,
/// $\u{205A}ecc\u{205A}0\u{205A}arg0\u{205A}t3}` whose induced per-element
/// INIT graph is acyclic and well-founded.
///
/// This is the deliberately-constructed AC3.1 coverage: the design notes
/// AC3.1 is the *only* coverage of the parent-`implicit_vars` sourcing
/// happy path, so the fixture shape was empirically verified (per design
/// deviation 4) to genuinely push a `$\u{205A}`-prefixed helper into a
/// *resolved* SCC -- this test re-asserts that here so a future converter
/// change that stopped synthesizing the in-SCC helper would fail loudly
/// rather than silently degrade AC3.1 to a no-op.
///
/// RED (structural, no destructive git): the synthetic helpers have NO
/// `SourceVariable` (they are absent from `model.variables`). Without
/// Phase 3 Task 2's parent-`implicit_vars` sourcing,
/// `var_phase_symbolic_fragment_prod`'s no-`SourceVariable` arm returns
/// `None` for each helper (the Task 1 loud-safe contract), so
/// `symbolic_phase_element_order`'s per-member `?` short-circuits the
/// whole builder to `None`, the `{ecc, $helper..}` SCC is `Unresolved`,
/// `has_cycle` stays `true`, and the model is rejected with
/// `CircularDependency` -- exactly the verdict the committed Task 1 test
/// `unsourceable_in_scc_node_falls_back_to_circular_no_panic` pins for an
/// unsourceable in-SCC node, and the verdict every non-helper-resolving
/// candidate shape produced while this fixture's shape was being
/// empirically selected. GREEN after Task 2 (committed `cee5d063`): the
/// helper's symbolic `PerVarBytecodes` is parent-sourced from `ecc`'s
/// `implicit_vars`, the SCC resolves, `has_cycle == false`, and it
/// simulates.
///
/// The test FIRST empirically confirms (tied to the REAL parsed fixture,
/// per the AC3.1 mandate) that the dependency graph carries exactly one
/// `ResolvedScc { phase: Initial }` whose members include a
/// `$\u{205A}`-prefixed synthetic helper that is genuinely
/// no-`SourceVariable` (absent from `model.variables`) yet resolves in
/// `model_implicit_var_info` -- i.e. the fixture genuinely exercises the
/// Task 2 parent-sourcing path and not merely an ordinary recurrence --
/// then simulates it via `simulate_mdl_path` against the hand-computed
/// `helper_recurrence.dat`. The recurrence has no time dependence
/// (`INITIAL` snapshots t=0; no stocks/PREVIOUS lag), so the values are
/// constant across both saved steps (INITIAL TIME=0, FINAL TIME=1, TIME
/// STEP=1 => 2 steps): ecc[t1]=1, ecc[t2]=INITIAL(ecc[t1]*2)=2,
/// ecc[t3]=INITIAL(ecc[t2]*2)=4.
#[test]
fn helper_recurrence_mdl_synthetic_helper_in_scc_simulates() {
    use simlin_engine::common::ErrorCode;
    use simlin_engine::db::{SccPhase, model_dependency_graph, model_implicit_var_info};

    let path = "../../test/sdeverywhere/models/helper_recurrence/helper_recurrence.mdl";
    let contents =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("missing fixture {path}: {e}"));
    let dm = open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &dm, None);
    let sync = sync.to_sync_result();
    let model = sync.models["main"].source;
    let dep_graph = model_dependency_graph(&db, model, sync.project);

    // (1) It compiles via the incremental path -- NO false
    // `CircularDependency`. RED before Task 2: the no-`SourceVariable`
    // helper is unsourceable -> SCC `Unresolved` -> `has_cycle` true ->
    // `CircularDependency`.
    assert!(
        !dep_graph.has_cycle,
        "helper_recurrence.mdl: the helper-bearing recurrence must NOT set \
         has_cycle once the synthetic helper is parent-sourced (AC3.1); \
         resolved_sccs = {:?}",
        dep_graph.resolved_sccs
    );
    let diags = collect_project_diagnostics(&dm);
    assert!(
        !diags
            .iter()
            .any(|d| diag_code(d) == Some(ErrorCode::CircularDependency)),
        "helper_recurrence.mdl: must NOT report CircularDependency once \
         the synthetic helper is sourced from its parent's implicit_vars \
         (AC3.1). Diagnostics: {diags:#?}"
    );

    // (2) Exactly one resolved SCC, init-phase (the `INITIAL()`-driven
    // recurrence is an init-relation cycle, not a dt one).
    assert_eq!(
        dep_graph.resolved_sccs.len(),
        1,
        "helper_recurrence.mdl: exactly one ResolvedScc (the \
         {{ecc, $helper..}} init cluster) -- got {:?}",
        dep_graph.resolved_sccs
    );
    let scc = &dep_graph.resolved_sccs[0];
    assert_eq!(
        scc.phase,
        SccPhase::Initial,
        "helper_recurrence.mdl: the resolved SCC MUST be phase == Initial \
         (the recurrence is driven by INITIAL(), an init-phase relation) \
         -- got {:?}",
        scc.phase
    );
    assert!(
        scc.members.len() >= 2,
        "helper_recurrence.mdl: AC3.1 requires the helper to be IN a \
         MULTI-member SCC alongside `ecc` (a 1-member SCC would mean the \
         helper is a mere forward dependency, not exercising the \
         in-SCC parent-sourcing path); got {} member(s): {:?}",
        scc.members.len(),
        scc.members
    );

    // (3) The CORE AC3.1 / Task 2 assertion: the resolved SCC genuinely
    // contains a `$\u{205A}`-prefixed synthetic-helper member that is
    // no-`SourceVariable` (absent from `model.variables`) yet resolves in
    // `model_implicit_var_info`. This is what makes the fixture exercise
    // the Task 2 parent-`implicit_vars` sourcing path rather than an
    // ordinary recurrence -- and is exactly the node whose symbolic
    // fragment is `None` (=> SCC `Unresolved` => `CircularDependency`)
    // without Task 2 (RED), `Some` (parent-sourced => SCC resolved) with
    // it (GREEN).
    let info = model_implicit_var_info(&db, model, sync.project);
    let source_vars = model.variables(&db);
    let in_scc_helpers: Vec<&str> = scc
        .members
        .iter()
        .map(|m| m.as_str())
        .filter(|m| {
            m.starts_with('$')
                && m.contains('\u{205A}')
                && source_vars.get(*m).is_none()
                && info.contains_key(*m)
        })
        .collect();
    assert!(
        !in_scc_helpers.is_empty(),
        "helper_recurrence.mdl: the resolved SCC MUST contain at least one \
         `$\u{205A}`-prefixed synthetic helper that has NO SourceVariable \
         (absent from model.variables) yet resolves in \
         model_implicit_var_info -- this is the node Phase 3 Task 2 \
         parent-sources; without it the fixture would not exercise the \
         AC3.1 happy path at all. SCC members: {:?}; implicit_var_info \
         keys: {:?}",
        scc.members,
        info.keys().collect::<Vec<_>>()
    );
    // Pin the parent: every in-SCC helper's `model_implicit_var_info`
    // entry must name `ecc` as its parent_source_var (the variable whose
    // `implicit_vars` Task 2 reaches), so the parent-sourcing chain the
    // test exercises is the intended one.
    for helper in &in_scc_helpers {
        let meta = &info[*helper];
        let parent = meta.parent_source_var.ident(&db);
        assert_eq!(
            parent.as_str(),
            "ecc",
            "in-SCC helper {helper:?} must be parented to `ecc` (the \
             recurrence variable whose implicit_vars Task 2 sources); \
             got parent {parent:?}"
        );
    }

    // (4) End-to-end: it compiles AND simulates to the hand-computed
    // helper_recurrence.dat. The synthetic helpers' symbolic
    // `PerVarBytecodes`, parent-sourced from `ecc`'s `implicit_vars`
    // (Task 2), are interleaved into the combined SCC fragment exactly
    // like a real member, producing the well-founded series
    // ecc[t1]=1, ecc[t2]=2, ecc[t3]=4 held constant across both saved
    // steps.
    simulate_mdl_path(path);
}

/// element-cycle-resolution.AC2.5 (Phase 2 Task 9, the transitioned
/// inter-variable-cycle assertion). `ref.mdl` and `interleaved.mdl` are
/// INTER-variable element-acyclic recurrence SCCs (`ce`<->`ecc` /
/// `a`<->`y`). Before Subcomponent B they were rejected with
/// `CircularDependency`; this test originally asserted exactly that.
///
/// Formerly named `ref_interleaved_inter_variable_cycles_report_circular`
/// (the Phase 1 form that asserted `CircularDependency`); this is the
/// AC2.5-transitioned successor of that test, renamed in Phase 2 Task 9.
/// The old name is recorded here so `git blame`/`git log -S` and a plain
/// grep on either name stay continuous across the rename.
/// Subcomponent B (GH #575) resolves the multi-member SCC and injects one
/// combined per-element fragment, so the fixtures now compile and
/// simulate.
///
/// Per AC2.5 this test is **transitioned** to its final end state: the
/// correct-simulation assertion is folded into the dedicated end-to-end
/// tests `ref_mdl_multi_variable_recurrence_simulates` (AC2.1, vs.
/// `ref.dat`) and `interleaved_mdl_element_interleave_simulates` (AC2.2,
/// vs. `interleaved.dat`) above -- those `simulate_mdl_path` tests ARE
/// the hand-computed-value proof, so re-asserting the series here would
/// be redundant. This test retains the still-meaningful diagnostic-level
/// guards the dedicated value tests do not pin:
///
///  1. The cycle-gate verdict transitioned correctly: NO
///     `CircularDependency` for either fixture, and the model compiles
///     end-to-end (the multi-member resolved SCC survives the dependency
///     graph -- the exact inverse of the original pre-Subcomponent-B
///     `CircularDependency` assertion this test made).
///  2. The AC2.5 leak guard, preserved verbatim from the original Phase 1
///     intent (the #559-class regression pin): the verdict change must
///     NOT spuriously inject a `self`/undefined-name
///     `UnknownDependency`/`DoesNotExist` leak into these
///     inter-variable-cycle fixtures. `ensure_results` in the value tests
///     would catch a wrong *number*, but only this guard pins the
///     specific "the cycle resolution leaked an undefined dependency
///     name" failure mode.
#[test]
fn ref_interleaved_inter_variable_cycles_simulate_no_circular_no_leak() {
    use simlin_engine::common::ErrorCode;
    for path in [
        "../../test/sdeverywhere/models/ref/ref.mdl",
        "../../test/sdeverywhere/models/interleaved/interleaved.mdl",
    ] {
        let mdl =
            std::fs::read_to_string(path).unwrap_or_else(|e| panic!("missing fixture {path}: {e}"));
        let (compile_err, diags) = compile_diags(&mdl);
        assert!(
            !compile_err,
            "{path}: Subcomponent B (GH #575) must let the element-acyclic \
             multi-member recurrence SCC compile end-to-end (correct \
             simulation is asserted by the dedicated `.dat` tests above). \
             Diagnostics: {diags:#?}"
        );
        assert!(
            !diags
                .iter()
                .any(|d| diag_code(d) == Some(ErrorCode::CircularDependency)),
            "{path}: the element-acyclic multi-member recurrence SCC must \
             survive the dependency graph -- it must NO LONGER report \
             CircularDependency (transitioned from the original \
             pre-Subcomponent-B assertion). Diagnostics: {diags:#?}"
        );
        assert!(
            !diags.iter().any(|d| matches!(
                diag_code(d),
                Some(ErrorCode::UnknownDependency) | Some(ErrorCode::DoesNotExist)
            )),
            "{path}: the cycle-gate verdict change must NOT inject a \
             `self`/undefined-name leak into a pure inter-variable-cycle \
             fixture (AC2.5 leak guard, preserved). Diagnostics: {diags:#?}"
        );
    }
}

/// Issue #559 (end-to-end). A 1D stock whose INTEG rate references a
/// higher-rank flow pinned to one element of the extra dimension
/// (`stock1d[scenario] = INTEG(fluxatm[scenario] - flux2d[scenario, L1], 0)`,
/// `flux2d[scenario, layers]`). The native MDL `collect_flows` dropped the
/// `[scenario, L1]` pin and wired the bare 2D `flux2d` as a named outflow
/// of the 1D stock, so the dimension checker reported
/// `mismatched_dimensions` on `stock1d` (e.g. C-LEARN's
/// `c_in_mixed_layer` / `heat_in_atmosphere_and_upper_ocean`).
///
/// Before the fix `compile_project_incremental` failed with a
/// `MismatchedDimensions` diagnostic on `stock1d`. After the fix the
/// rank-changing subscripted reference falls through to the synthetic
/// net-flow path (which preserves the 1D `flux2d[scenario, L1]` slice in
/// the rate), so the model compiles and simulates. `fluxatm`(2) -
/// `flux2d[*,L1]`(1) = 1 per step into `stock1d` (init 0): [0, 1].
#[test]
fn simulates_subscript_pinned_higher_rank_flow() {
    use simlin_engine::common::ErrorCode;
    let mdl = "\
{UTF-8}
scenario: s1, s2 ~~|
layers: (L1-L4) ~~|
flux2d[scenario, layers] = 1 ~~|
fluxatm[scenario] = 2 ~~|
stock1d[scenario] = INTEG(fluxatm[scenario] - flux2d[scenario, L1], 0) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 1 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let (compile_err, diags) = compile_diags(mdl);
    // The bug surfaces specifically as MismatchedDimensions on the 1D
    // stock fed by the mis-wired bare 2D flow.
    assert!(
        !diags
            .iter()
            .any(|d| diag_code(d) == Some(ErrorCode::MismatchedDimensions)),
        "a subscript-pinned higher-rank flow must not produce \
         MismatchedDimensions on the 1D stock. Diagnostics: {diags:#?}"
    );
    assert!(
        !compile_err,
        "stock1d with a pinned-slice flow must compile (synthetic \
         net-flow preserves the 1D `flux2d[scenario, L1]` slice). \
         Diagnostics: {diags:#?}"
    );
    // It simulates: stock1d[s] = INTEG(fluxatm(2) - flux2d[s,L1](1)) per
    // scenario element, init 0 -> [0, 1] over the 2 steps.
    let r = run_inline_mdl(mdl);
    assert_eq!(r.step_count, 2);
    assert_eq!(element_series(&r, "stock1d[s1]"), vec![0.0, 1.0]);
    assert_eq!(element_series(&r, "stock1d[s2]"), vec![0.0, 1.0]);
}

/// Issue #559 (end-to-end; e.g. C-LEARN's `c_in_deep_ocean_net_flow` /
/// `heat_in_deep_ocean_net_flow`). A 2-D stock over a shift-mapped
/// subrange whose differing per-equation flow lists force a synthesized
/// `<stock>_net_flow`. `build_synthetic_flow_equation` cloned the RAW
/// subrange-sliced rate (`dflux[scenario, upper] - dflux[scenario, lower]`)
/// into every scalar element key instead of resolving it per element, so
/// the dimension checker reported `MismatchedDimensions` on
/// `stock2d_net_flow`.
///
/// After the per-element resolution fix, that `MismatchedDimensions` is
/// gone (the parser-level shape is correct).
///
/// The end-to-end deep-ocean *value* check is deferred: the C-LEARN
/// deep-ocean is a genuine subscript-shift recurrence (`dflux` depends on
/// the stock) that only *simulates* once element-level cycle resolution
/// also lands (#559). This minimal repro's `dflux` is constant, so it has
/// no such cycle; we therefore assert ONLY the parser-level symptom
/// removal (no `MismatchedDimensions` on the synthesized net-flow) and
/// explicitly do NOT assert deep-ocean simulation *values* here.
#[test]
fn simulates_synthetic_net_flow_shape() {
    use simlin_engine::common::ErrorCode;
    let mdl = "\
{UTF-8}
scenario: s1, s2 ~~|
layers: (L1-L4) ~~|
upper: (L1-L3) -> lower ~~|
lower: (L2-L4) -> upper ~~|
bottom: L4 ~~|
dflux[scenario, L1] = 5 ~~|
dflux[scenario, lower] = 6 ~~|
stock2d[scenario, upper] = INTEG(dflux[scenario, upper] - dflux[scenario, lower], 0) ~~|
stock2d[scenario, bottom] = INTEG(dflux[scenario, bottom], 0) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 1 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let (_compile_err, diags) = compile_diags(mdl);
    // The bug surfaces as MismatchedDimensions on the synthesized
    // `_net_flow` (raw subrange-sliced rate cloned into scalar element
    // keys). After the fix that specific symptom is gone.
    let mismatched: Vec<_> = diags
        .iter()
        .filter(|d| diag_code(d) == Some(ErrorCode::MismatchedDimensions))
        .collect();
    assert!(
        mismatched.is_empty(),
        "the synthesized net-flow must be resolved per element so no \
         MismatchedDimensions remains (parser-level shape fix). Found: \
         {mismatched:#?}"
    );
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

// --- macros.AC6.3: focused C-LEARN-macro isolation fixtures ----------------
//
// Each of C-LEARN's three *invoked* macros (`SAMPLE UNTIL`, `SSHAPE`,
// `RAMP FROM TO`) is exercised by a small focused `.mdl` whose `:MACRO:`
// block is copied VERBATIM from `C-LEARN v77 for Vensim.mdl` and invoked
// with known constant inputs (>= 2 args so the call is not rewritten to
// `LOOKUP` -- GH #553). The expected `output.tab` is hand-computed by
// applying the macro body formula to those inputs (worked out in each
// fixture's README.md, grounded in the engine's `STEP`/`RAMP`/`INTEG`
// semantics). C-LEARN's uninvoked `INIT` macro needs no focused model --
// macros.AC6.2's "parse, register, expand" (`corpus_clearn_macros_import`)
// covers it (the macros.AC1.7 "defined but never invoked" case).
//
// No Vensim DSS reference `.vdf` is checked in for these focused fixtures
// (authoring one is a documented prerequisite/setup task per the design's
// "Test prerequisites" note, not implementation work); the formula-derived
// `output.tab` is the gate. `simulate_mdl_path` prefers `output.tab`/`.dat`
// already -- if a `.vdf` is later added, a `.vdf`-aware path would prefer it.

/// macros.AC6.3 -- C-LEARN's `SAMPLE UNTIL` macro (a stock that tracks
/// `input` until `lastTime`, then FREEZES) computes its defined behavior
/// on a *time-varying* input: `SAMPLE UNTIL(4, 5+RAMP(1,0,10), 99)` =
/// `[99, 5, 6, 7, 8, 8, 8, 8, 8]` over t = 0..8 -- it tracks the rising
/// input through t=3, then holds the sampled 8 (distinct from both the
/// init 99 and every later input 9..13, so the freeze is discriminating;
/// see the fixture README for the full derivation).
#[test]
fn simulates_macro_clearn_sample_until_mdl() {
    simulate_mdl_path(
        "../../test/test-models/tests/macro_clearn_sample_until/test_macro_clearn_sample_until.mdl",
    );
}

/// macros.AC6.3 -- C-LEARN's `SSHAPE` macro (an S-curve with a macro-local
/// `input = MIN(1, MAX(0, xin))` clamp) computes its defined behavior on
/// both `IF THEN ELSE` branches: `SSHAPE(0.8, 2) = 0.92` (upper),
/// `SSHAPE(0.3, 2) = 0.18` (lower).
#[test]
fn simulates_macro_clearn_sshape_mdl() {
    simulate_mdl_path(
        "../../test/test-models/tests/macro_clearn_sshape/test_macro_clearn_sshape.mdl",
    );
}

/// macros.AC6.3 -- C-LEARN's `RAMP FROM TO` macro (a 7-body-variable
/// from/to ramp) computes its defined behavior on the linear branch:
/// `RAMP FROM TO(2, 10, 1, 5, 1)` = `[2, 2, 4, 6, 8, 10, 10]` over
/// t = 0..6 (a clamped linear ramp from 2 to 10).
#[test]
fn simulates_macro_clearn_ramp_from_to_mdl() {
    simulate_mdl_path(
        "../../test/test-models/tests/macro_clearn_ramp_from_to/test_macro_clearn_ramp_from_to.mdl",
    );
}

/// A macro-marked model's definition, reduced to the parts that must survive
/// a cross-format conversion: its name, its `MacroSpec`, and its body
/// variables as `(ident, equation)` pairs sorted by ident (so the comparison
/// is order-independent).
#[derive(Debug, Clone, PartialEq)]
struct MacroDef {
    name: String,
    spec: simlin_engine::datamodel::MacroSpec,
    body: Vec<(String, Option<simlin_engine::datamodel::Equation>)>,
}

/// Collect every macro-marked model in `project` as a `MacroDef`, sorted by
/// macro name.
fn collect_macro_defs(project: &simlin_engine::datamodel::Project) -> Vec<MacroDef> {
    let mut defs: Vec<MacroDef> = project
        .models
        .iter()
        .filter_map(|m| {
            m.macro_spec.as_ref().map(|spec| {
                let mut body: Vec<(String, Option<simlin_engine::datamodel::Equation>)> = m
                    .variables
                    .iter()
                    .map(|v| (v.get_ident().to_string(), v.get_equation().cloned()))
                    .collect();
                body.sort_by(|a, b| a.0.cmp(&b.0));
                MacroDef {
                    name: m.name.clone(),
                    spec: spec.clone(),
                    body,
                }
            })
        })
        .collect();
    defs.sort_by(|a, b| a.name.cmp(&b.name));
    defs
}

/// macros.AC4.4: a single-output macro survives a cross-format conversion
/// `.mdl` -> datamodel -> `.xmile` -> datamodel. We `open_vensim` a
/// single-output macro `.mdl` fixture, convert the resulting
/// `datamodel::Project` to XMILE via `to_xmile`, re-import it via
/// `open_xmile`, and assert the macro definition (the macro-marked `Model` +
/// its `MacroSpec`) and the invocation are preserved -- the
/// cross-format-round-tripped project's macro models and invocation equations
/// match those of the directly-imported `.mdl` project.
#[test]
fn macro_cross_format_mdl_to_xmile_to_datamodel_preserves_macro() {
    let path = "../../test/test-models/tests/macro_expression/test_macro_expression.mdl";
    let contents =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));

    // .mdl -> datamodel (the reference shape).
    let from_mdl = open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));

    // datamodel -> .xmile -> datamodel (the cross-format round-trip).
    let xmile_str =
        simlin_engine::to_xmile(&from_mdl).unwrap_or_else(|e| panic!("to_xmile failed: {e}"));
    let cross_rt = {
        let mut reader = BufReader::new(xmile_str.as_bytes());
        simlin_engine::open_xmile(&mut reader)
            .unwrap_or_else(|e| panic!("open_xmile of the converted XMILE failed: {e}"))
    };

    let mdl_defs = collect_macro_defs(&from_mdl);
    let rt_defs = collect_macro_defs(&cross_rt);

    // There IS a macro definition, and it is preserved exactly across the
    // cross-format conversion (name, MacroSpec, and body equations).
    assert!(
        !mdl_defs.is_empty(),
        "the .mdl fixture must import at least one macro-marked model"
    );
    assert_eq!(
        mdl_defs, rt_defs,
        "the macro definition (macro-marked Model + MacroSpec + body) must \
         survive the .mdl -> .xmile -> datamodel cross-format conversion"
    );

    // The invocation is preserved: the `main` model's invocation aux reads
    // the same macro call equation before and after the cross-format trip.
    let invocation_eqn = |p: &simlin_engine::datamodel::Project| -> String {
        let main = p.get_model("main").expect("project has a `main` model");
        let v = main
            .get_variable("macro output")
            .expect("`main` has the `macro output` invocation variable");
        match v.get_equation() {
            Some(simlin_engine::datamodel::Equation::Scalar(s)) => s.clone(),
            other => panic!("expected a scalar invocation equation, got {other:?}"),
        }
    };
    let mdl_inv = invocation_eqn(&from_mdl);
    let rt_inv = invocation_eqn(&cross_rt);
    assert_eq!(
        mdl_inv, rt_inv,
        "the macro invocation equation must survive the cross-format \
         conversion (mdl: {mdl_inv:?}, round-tripped: {rt_inv:?})"
    );
    // And it really is an invocation of the imported macro.
    let macro_name = &mdl_defs[0].name;
    assert!(
        mdl_inv.to_lowercase().contains(macro_name.as_str()),
        "the invocation {mdl_inv:?} must call the macro {macro_name:?}"
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

// ===========================================================================
// Phase 4 / Task 1: multi-output (`:`-list) macro invocation
//
// A multi-output invocation `total = ADD3(in1, in2, in3 : the min, the max)`
// materializes at MDL import as a Variable::Module plus binding Auxes (the
// LHS reads the primary output; the `:`-list names read the additional
// outputs). The fixture is stockless so every value is constant; its
// `output.tab` lists `total`, `the min`, `the max`, and the downstream
// `spread = the max - the min` (which proves macros.AC3.2: the `:`-list names
// are referenceable by a subsequent equation and carry correct values).
//
//   total  = in1 + in2 + in3    = 7 + 2 + 5         = 14
//   the min = MIN(7, MIN(2, 5)) = 2
//   the max = MAX(7, MAX(2, 5)) = 7
//   spread  = the max - the min = 7 - 2             = 5
// ===========================================================================

/// macros.AC3.1 / macros.AC3.2: the bundled multi-output fixture parses,
/// materializes, compiles, simulates, and matches its hand-computed
/// `output.tab` (`total`/`the min`/`the max`/`spread`).
#[test]
fn simulates_macro_multi_output_mdl() {
    simulate_mdl_path(
        "../../test/test-models/tests/macro_multi_output/test_macro_multi_output.mdl",
    );
}

// ===========================================================================
// Phase 4 / Task 2: arrayed (apply-to-all) macro invocation
//
// Phase 3 made `instantiate_implicit_modules`'s apply-to-all path
// macro-aware (`contains_module_call`), so an arrayed macro invocation
// `out[Region] = SCALE(inp[Region], factor)` rides the EXISTING per-element
// module-expansion machinery -- one independent synthetic Variable::Module
// per dimension element -- with no new mechanism. These tests verify that
// (macros.AC3.4) and the per-element-independent-stock edge (macros.AC3.5).
//
// SCALE / ACCUM each take >= 2 parameters: a 1-arg MDL call `NAME(arg)` is
// rewritten to LOOKUP before macro resolution (GH #553).
// ===========================================================================

/// macros.AC3.4: the bundled arrayed fixture parses, expands per-element,
/// compiles, simulates, and matches its hand-computed `output.tab`
/// (`out[R1]=30`, `out[R2]=60`, `out[R3]=90` = `inp[element] * factor`).
#[test]
fn simulates_macro_arrayed_mdl() {
    simulate_mdl_path("../../test/test-models/tests/macro_arrayed/test_macro_arrayed.mdl");
}

/// macros.AC3.4 (expansion-level): the arrayed invocation
/// `out[Region] = SCALE(inp[Region], factor)` must expand into one
/// *independent* synthetic `Variable::Module` PER `Region` element
/// (subscript-suffixed idents), not a single shared instance. We assert
/// this through the full compile pipeline by inspecting the compiled
/// `Results.offsets`: each per-element macro instance contributes its own
/// `$⁚out⁚{n}⁚scale⁚{elem}·scale` body-output slot.
#[test]
fn arrayed_macro_invocation_expands_one_module_per_element() {
    let path = "../../test/test-models/tests/macro_arrayed/test_macro_arrayed.mdl";
    let contents =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    let datamodel_project =
        open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));
    let compiled = compile_vm(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed: {e}"));
    let results = vm.into_results();

    // One synthetic macro-instance per Region element: the per-element
    // module's primary-output body slot is `$⁚out⁚{n}⁚scale⁚{elem}·scale`.
    // Collect the distinct `{elem}` suffixes.
    let mut per_element_instances: Vec<String> = results
        .offsets
        .keys()
        .filter_map(|k| {
            let s = k.as_str();
            // $⁚out⁚<n>⁚scale⁚<elem>·scale
            let rest = s.strip_prefix("$\u{205a}out\u{205a}")?;
            let rest = rest.split_once('\u{205a}')?.1; // drop the `<n>⁚`
            let elem = rest.strip_prefix("scale\u{205a}")?;
            let elem = elem.strip_suffix("\u{b7}scale")?;
            Some(elem.to_string())
        })
        .collect();
    per_element_instances.sort();
    per_element_instances.dedup();
    assert_eq!(
        per_element_instances,
        vec!["r1".to_string(), "r2".to_string(), "r3".to_string()],
        "the arrayed SCALE invocation must expand into one independent macro \
         instance per Region element (subscript-suffixed), not a shared one; \
         all offsets: {:?}",
        results
            .offsets
            .keys()
            .map(|k| k.as_str().to_string())
            .collect::<Vec<_>>()
    );

    // And the arrayed result itself has one slot per element with the
    // hand-computed value (inp[element] * factor).
    for (elem, expected) in [("r1", 30.0), ("r2", 60.0), ("r3", 90.0)] {
        let v = macro_test_value_at(&results, &format!("out[{elem}]"), 0);
        assert!(
            (v - expected).abs() < 1e-9,
            "out[{elem}] = {v}, expected {expected} (inp[{elem}] * factor)"
        );
    }
}

/// macros.AC3.5: an arrayed invocation of a *stock-bearing* macro gives each
/// dimension element its own persistent stock. Macro
/// `ACCUM(rate, init) = INTEG(rate, init)`, invoked
/// `total[Region] = ACCUM(rate[Region], 0)` with `rate = [1, 3]`.
///
/// Vensim INTEG: value at step k (t = k) is `init + rate*k`. With
/// INITIAL TIME=0, FINAL TIME=4, TIME STEP=1 (5 steps, t=0..4):
///   total[R1] = 0, 1, 2, 3, 4    (its own rate = 1)
///   total[R2] = 0, 3, 6, 9, 12   (its own rate = 3)
/// If the elements shared one stock these series could not differ -- each
/// element integrating its OWN rate proves per-element persistent state.
#[test]
fn simulates_arrayed_macro_per_element_independent_stock() {
    let mdl = "\
{UTF-8}
:MACRO: ACCUM(rate, init)
ACCUM = INTEG(rate, init)
	~	dmnl
	~	per-element independent persistent stock
	|
:END OF MACRO:
Region: R1, R2 ~~|
rate[Region]= 1, 3 ~~|
total[Region]= ACCUM(rate[Region], 0) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 4 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let results = run_inline_mdl(mdl);
    let expected_r1 = [0.0, 1.0, 2.0, 3.0, 4.0];
    let expected_r2 = [0.0, 3.0, 6.0, 9.0, 12.0];
    for step in 0..5 {
        let r1 = macro_test_value_at(&results, "total[r1]", step);
        let r2 = macro_test_value_at(&results, "total[r2]", step);
        assert!(
            (r1 - expected_r1[step]).abs() < 1e-9,
            "step {step}: total[r1] = {r1}, expected {} (ACCUM with its own rate=1)",
            expected_r1[step]
        );
        assert!(
            (r2 - expected_r2[step]).abs() < 1e-9,
            "step {step}: total[r2] = {r2}, expected {} (ACCUM with its own rate=3)",
            expected_r2[step]
        );
    }
}

// ===========================================================================
// Phase 4 / Task 3: early validation against the multi-output / arrayed
// corpus models (THEIL, SSTATS, C-LEARN). Tests only -- a focused early
// gate; the full tiered corpus harness + Vensim-reference comparison is
// Phase 7. The two heavy models (the SSTATS COVID model; C-LEARN, ~53k
// lines) are #[ignore]d with a documented opt-in per the rust.md
// test-time-budget rules; Theil_2011.mdl compiles+runs in ~40ms so it
// stays a regular test.
// ===========================================================================

/// All `Diagnostic`s for a datamodel project, via the salsa pipeline.
fn collect_project_diagnostics(
    dm: &simlin_engine::datamodel::Project,
) -> Vec<simlin_engine::db::Diagnostic> {
    use simlin_engine::db::{
        SimlinDb, collect_all_diagnostics, compile_project_incremental,
        sync_from_datamodel_incremental,
    };
    let mut db = SimlinDb::default();
    let sync_state = sync_from_datamodel_incremental(&mut db, dm, None);
    let sync = sync_state.to_sync_result();
    // Drive compilation so the diagnostic accumulators are populated; the
    // Result is intentionally ignored here (callers inspect diagnostics).
    let _ = compile_project_incremental(&db, sync.project, "main");
    collect_all_diagnostics(&db, &sync)
}

/// Pure predicate: is `equation` EXACTLY the `{module_ident}.{output}`
/// binding form a materialized multi-output aux carries?
///
/// Materialization emits `Equation::Scalar(format!("{module_ident}.{output}"))`
/// verbatim, where `output` is a bare macro-output identifier
/// (`spec.primary_output` / `spec.additional_outputs[i]`). The match is
/// therefore exact, not a prefix: split on the FIRST ASCII `.`; the part
/// before it must equal `module_ident` exactly, and the remainder must be a
/// *single bare identifier token* -- non-empty and composed solely of
/// canonical-identifier characters (ASCII alphanumeric or `_`).
///
/// The first-period split plus the identifier-only suffix check together
/// reject anything that is not the verbatim binding text: a hypothetical
/// multi-segment reference like `mod.sub.out` (the suffix `sub.out`
/// contains `.`), an unrelated aux that merely *references* a module output
/// inside a larger expression (`mod.out + 1` -- the suffix `out + 1`
/// contains spaces and `+`), and a different module's output
/// (`other_mod.out` -- the prefix is not `module_ident`). This avoids the
/// prior `starts_with("{mi}.")` over-count while making the predicate as
/// precise as the materialized form it recognizes.
fn is_module_output_binding(equation: &str, module_ident: &str) -> bool {
    let Some((prefix, suffix)) = equation.split_once('.') else {
        return false;
    };
    prefix == module_ident
        && !suffix.is_empty()
        && suffix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Count a model's `Variable::Module`s whose `model_name` is `macro_model`,
/// plus the binding `Aux`es whose Scalar equation is EXACTLY the
/// `<module>.<output>` binding form (ASCII period -- the datamodel
/// separator). Returns `(module_count, binding_aux_count)`.
fn count_materialized_macro(
    project: &simlin_engine::datamodel::Project,
    macro_model: &str,
) -> (usize, usize) {
    use simlin_engine::datamodel::{Equation, Variable};
    let main = project.get_model("main").expect("project has a main model");
    let module_idents: Vec<String> = main
        .variables
        .iter()
        .filter_map(|v| match v {
            Variable::Module(m) if m.model_name == macro_model => Some(m.ident.clone()),
            _ => None,
        })
        .collect();
    let binding_auxes = main
        .variables
        .iter()
        .filter(|v| match v {
            Variable::Aux(a) => match &a.equation {
                Equation::Scalar(s) => module_idents
                    .iter()
                    .any(|mi| is_module_output_binding(s, mi)),
                _ => false,
            },
            _ => false,
        })
        .count();
    (module_idents.len(), binding_auxes)
}

/// macros.AC3.3 -- THEIL. The metasd Theil model's 2-input/13-output
/// `THEIL` multi-output invocation materializes (one `Variable::Module` +
/// 1 primary + 13 additional binding `Aux`es), compiles, and runs to the
/// end. ~40ms total, so this is a regular (non-ignored) test.
#[test]
fn corpus_theil_multi_output_materializes_and_simulates() {
    let path = "../../test/metasd/theil-statistics/Theil_2011.mdl";
    let contents =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    let dm = open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));

    // THEIL macro imported with the correct 2-input/13-output spec.
    let theil = dm
        .models
        .iter()
        .find(|m| m.name == "theil" && m.macro_spec.is_some())
        .expect("THEIL macro must import as a macro-marked model");
    let spec = theil.macro_spec.as_ref().unwrap();
    assert_eq!(spec.parameters, vec!["historical", "simulated"]);
    assert_eq!(
        spec.additional_outputs.len(),
        13,
        "THEIL has 13 `:`-outputs"
    );

    // The multi-output invocation materialized: one Module + (1 primary +
    // 13 additional) binding auxes = 14.
    let (modules, bindings) = count_materialized_macro(&dm, "theil");
    assert_eq!(modules, 1, "exactly one THEIL module instance");
    assert_eq!(
        bindings, 14,
        "1 primary + 13 additional THEIL binding auxes"
    );

    // It compiles and runs to the end.
    let compiled = compile_vm(&dm);
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("THEIL VM creation failed: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("THEIL VM run failed: {e}"));
    let _ = vm.into_results();
}

/// macros.AC3.3 -- SSTATS. The metasd COVID model's two
/// 2-input/10-output `SSTATS` invocations both materialize (each: one
/// `Variable::Module` + 1 primary + 10 additional binding `Aux`es).
///
/// This large real-world COVID model has UNRELATED, non-macro blockers
/// that prevent it reaching a runnable VM: its `*_data` variables are
/// unresolved `GET DIRECT/GET XLS DATA` references (no DataProvider /
/// data files are supplied here), so they compile to
/// `EmptyEquation`/`UnknownBuiltin` and `compile_project_incremental`
/// returns `not_simulatable`. Per the phase plan, the assertion is
/// therefore narrowed to "SSTATS multi-output materialization succeeded
/// and produced no macro-specific compile diagnostics"; the unrelated
/// GET-DIRECT-data blocker is reported for Phase-7 tiered-harness scope.
///
/// `#[ignore]` (large COVID model).
// Run with: cargo test --release -- --ignored corpus_sstats_multi_output_materializes
#[test]
#[ignore]
fn corpus_sstats_multi_output_materializes() {
    let path = "../../test/metasd/covid19-us-homer/homer v8/Covid19US v8.mdl";
    let contents =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    let dm = open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));

    // SSTATS macro imported with the correct 2-input/10-output spec.
    let sstats = dm
        .models
        .iter()
        .find(|m| m.name == "sstats" && m.macro_spec.is_some())
        .expect("SSTATS macro must import as a macro-marked model");
    let spec = sstats.macro_spec.as_ref().unwrap();
    assert_eq!(spec.parameters, vec!["historical", "simulated"]);
    assert_eq!(
        spec.additional_outputs.len(),
        10,
        "SSTATS has 10 `:`-outputs"
    );

    // BOTH SSTATS invocations materialized: 2 Module instances, each with
    // 1 primary + 10 additional binding auxes => 2 modules, 22 bindings.
    let (modules, bindings) = count_materialized_macro(&dm, "sstats");
    assert_eq!(
        modules, 2,
        "both SSTATS invocations must materialize as module instances"
    );
    assert_eq!(
        bindings, 22,
        "2 invocations x (1 primary + 10 additional) SSTATS binding auxes"
    );

    // No macro-specific compile diagnostic (the COVID model's only
    // blockers are the unrelated unresolved `*_data` GET-DIRECT
    // references; the SSTATS macro itself must not produce
    // UnknownBuiltin/BadModelName/BadBuiltinArgs/DuplicateMacroName).
    use simlin_engine::common::ErrorCode;
    use simlin_engine::db::DiagnosticError;
    let macro_codes = [
        ErrorCode::UnknownBuiltin,
        ErrorCode::BadModelName,
        ErrorCode::BadBuiltinArgs,
        ErrorCode::DuplicateMacroName,
        ErrorCode::CircularDependency,
    ];
    for d in collect_project_diagnostics(&dm) {
        let code = match &d.error {
            DiagnosticError::Equation(e) => Some(e.code),
            DiagnosticError::Model(e) => Some(e.code),
            _ => None,
        };
        if let Some(c) = code
            && macro_codes.contains(&c)
        {
            // The unrelated `*_data` GET-DIRECT references are the only
            // legitimate UnknownBuiltin/EmptyEquation sources; assert the
            // diagnostic is on such a variable, not on the SSTATS macro.
            let var = d.variable.clone().unwrap_or_default();
            assert!(
                var.ends_with("_data") || var.contains("_data"),
                "unexpected macro-specific diagnostic NOT on an unrelated \
                 `*_data` GET-DIRECT variable: model={} var={:?} {:?}",
                d.model,
                d.variable,
                d.error
            );
        }
    }
}

/// Return every *macro-attributable* diagnostic in `diags` for the
/// already-imported datamodel `dm` -- diagnostics that indicate macro
/// handling itself (registration, expansion, body compilation) failed, as
/// opposed to an unrelated non-macro model blocker.
///
/// After Phases 1-6 a *correctly* macro-using model produces ZERO
/// macro-attributable diagnostics: single-output macro invocations are
/// inlined into the caller and multi-output ones materialize as ordinary
/// `Variable::Module`s + binding auxes, so the only diagnostics a working
/// macro pipeline can emit are unrelated (model-logic / unit / dimension)
/// blockers. There is no macro-specific `ErrorCode`, so a diagnostic is
/// macro-attributable iff ANY of:
///
/// 1. **Macro-registry-build error** -- a project-level (`model` empty,
///    `variable` `None`) `Model` diagnostic with code `CircularDependency`
///    or `DuplicateMacroName`. `db_macro_registry::project_macro_registry`
///    emits this when `MacroRegistry::build` rejects the macro set; an empty
///    registry then un-shadows every macro builtin (the #554 cascade:
///    `SSHAPE`/`SAMPLE UNTIL`/`RAMP FROM TO` calls become
///    `BadBuiltinArgs`/`UnknownBuiltin`). This is distinct from a
///    *model-logic* circular dependency, which is attributed to a
///    model/variable.
/// 2. **Macro-template-body Error** -- an `Error`-severity diagnostic whose
///    `model` is a macro-marked model's name (its `macro_spec` is `Some`).
///    The macro body failed to compile/expand. Unit-inference *warnings* on
///    a macro body (formal-parameter port variables legitimately have no
///    units) are `Warning` severity and are an allowed non-macro unit-error
///    blocker, so they are excluded.
/// 3. **Macro-resolution-failure code** -- a diagnostic whose code is
///    `UnknownBuiltin`/`BadBuiltinArgs`/`BadModelName`/`DuplicateMacroName`
///    AND whose `model` is a macro-marked model OR which is project-level. A
///    bare `UnknownBuiltin`/`UnknownDependency`/`DoesNotExist` on an
///    ordinary `main` variable (an unrelated builtin, a model-logic
///    dependency, or Phase 3's deprioritized non-time `$` reference -- which
///    surfaces as an *ordinary* unresolved-reference diagnostic) is NOT
///    macro-attributable; the classifier must not mistake it for a macro
///    error.
fn macro_attributable_diagnostics<'a>(
    dm: &simlin_engine::datamodel::Project,
    diags: &'a [simlin_engine::db::Diagnostic],
) -> Vec<&'a simlin_engine::db::Diagnostic> {
    use simlin_engine::common::ErrorCode;
    use simlin_engine::db::{DiagnosticError, DiagnosticSeverity};

    let macro_models: std::collections::BTreeSet<&str> = dm
        .models
        .iter()
        .filter(|m| m.macro_spec.is_some())
        .map(|m| m.name.as_str())
        .collect();

    // Macro-resolution-failure codes: the symptoms of a macro call that did
    // not resolve to its macro (the registry was empty / the macro name was
    // not registered), so the call site fell through to builtin/module
    // resolution and failed there.
    let resolution_codes = [
        ErrorCode::UnknownBuiltin,
        ErrorCode::BadBuiltinArgs,
        ErrorCode::BadModelName,
        ErrorCode::DuplicateMacroName,
    ];

    let code_of = |d: &simlin_engine::db::Diagnostic| match &d.error {
        DiagnosticError::Equation(e) => Some(e.code),
        DiagnosticError::Model(e) => Some(e.code),
        _ => None,
    };
    let is_registry_build_error = |d: &simlin_engine::db::Diagnostic| {
        d.model.is_empty()
            && d.variable.is_none()
            && matches!(&d.error, DiagnosticError::Model(_))
            && matches!(
                code_of(d),
                Some(ErrorCode::CircularDependency) | Some(ErrorCode::DuplicateMacroName)
            )
    };

    // The #554 cascade is *defined by* a registry-build error: when present,
    // every macro call un-shadows and fails with a resolution-failure code.
    // So a resolution-failure code is macro-attributable when it co-occurs
    // with a registry-build error (the cascade), even on a `main` variable.
    // Absent a registry error, a lone resolution-failure code on an ordinary
    // `main` variable is an unrelated builtin/model issue, not a macro error.
    let registry_error_present = diags.iter().any(&is_registry_build_error);

    diags
        .iter()
        .filter(|d| {
            let code = code_of(d);
            let is_project_level = d.model.is_empty() && d.variable.is_none();
            let in_macro_model = macro_models.contains(d.model.as_str());

            // (1) Macro-registry-build error (the #554 cascade class).
            let registry_build_error = is_registry_build_error(d);

            // (2) Error-severity diagnostic inside a macro template body
            // (unit *warnings* on a macro body are an allowed non-macro
            // unit-error blocker -- excluded by the severity check).
            let macro_body_error = in_macro_model && d.severity == DiagnosticSeverity::Error;

            // (3) Macro-resolution-failure code on a macro model, or
            // project-level, or co-occurring with a registry-build error
            // (the #554 cascade). A bare such code on an ordinary `main`
            // variable with no registry error is an unrelated blocker.
            let resolution_failure = code.map(|c| resolution_codes.contains(&c)).unwrap_or(false)
                && (in_macro_model || is_project_level || registry_error_present);

            registry_build_error || macro_body_error || resolution_failure
        })
        .collect()
}

/// The macro-attributable classifier must (a) flag the three macro-error
/// shapes and (b) NOT flag C-LEARN's allowed non-macro blockers
/// (model-logic `CircularDependency` on a variable, dimension mismatch,
/// non-time `$` unresolved reference, a unit *warning* on a macro body).
/// This pins the classifier so neither the C-LEARN nor the metasd harness
/// assertion can silently degrade into "flags everything" or "flags
/// nothing". Uses a real macro-marked datamodel (a tiny inline `.mdl`) so
/// it is not brittle to `datamodel::Project` struct changes.
#[test]
fn macro_attributable_classifier_separates_macro_from_nonmacro() {
    use simlin_engine::common::{Error, ErrorCode, ErrorKind};
    use simlin_engine::db::{Diagnostic, DiagnosticError, DiagnosticSeverity};

    // A real macro-marked model named `m` (single-output macro `M`).
    let dm = open_vensim(
        "{UTF-8}\n\
         :MACRO: M(a, b)\n\
         M = a * b\n\t~\tdmnl\n\t~\t|\n\
         :END OF MACRO:\n\
         x= M(2, 3) ~~|\n\
         INITIAL TIME = 0 ~~|\n\
         FINAL TIME = 1 ~~|\n\
         SAVEPER = 1 ~~|\n\
         TIME STEP = 1 ~~|\n",
    )
    .expect("inline macro mdl parses");
    assert!(
        dm.models.iter().any(|m| m.macro_spec.is_some()),
        "fixture must have a macro-marked model"
    );
    let macro_model = dm
        .models
        .iter()
        .find(|m| m.macro_spec.is_some())
        .unwrap()
        .name
        .clone();

    let eq = |code: ErrorCode| {
        DiagnosticError::Equation(simlin_engine::common::EquationError {
            start: 0,
            end: 0,
            code,
        })
    };
    let model_err =
        |code: ErrorCode| DiagnosticError::Model(Error::new(ErrorKind::Model, code, None));

    // --- (a) The three macro-error shapes MUST be flagged ---
    let registry_build = Diagnostic {
        model: String::new(),
        variable: None,
        error: model_err(ErrorCode::CircularDependency),
        severity: DiagnosticSeverity::Error,
    };
    let macro_body_error = Diagnostic {
        model: macro_model.clone(),
        variable: Some("m".to_string()),
        error: eq(ErrorCode::UnknownDependency),
        severity: DiagnosticSeverity::Error,
    };
    for d in [&registry_build, &macro_body_error] {
        let flagged = macro_attributable_diagnostics(&dm, std::slice::from_ref(d));
        assert_eq!(
            flagged.len(),
            1,
            "this diagnostic must be macro-attributable: {d:?}"
        );
    }
    // The #554 cascade: a registry-build error PLUS the resulting
    // `UnknownBuiltin` on the macro-invoking `main` variable. Both must be
    // flagged (the resolution failure is macro-attributable *because* the
    // registry error is present).
    let cascade_resolution_failure = Diagnostic {
        model: "main".to_string(),
        variable: Some("x".to_string()),
        error: eq(ErrorCode::UnknownBuiltin),
        severity: DiagnosticSeverity::Error,
    };
    let cascade = [registry_build.clone(), cascade_resolution_failure.clone()];
    let flagged = macro_attributable_diagnostics(&dm, &cascade);
    assert_eq!(
        flagged.len(),
        2,
        "the #554 cascade (registry-build error + the resulting \
         `UnknownBuiltin` on the macro-invoking variable) must BOTH be \
         macro-attributable; flagged: {flagged:#?}"
    );
    // But that same `UnknownBuiltin` on `main.x` ALONE (no registry error)
    // is an unrelated builtin issue -- NOT macro-attributable.
    let lone =
        macro_attributable_diagnostics(&dm, std::slice::from_ref(&cascade_resolution_failure));
    assert!(
        lone.is_empty(),
        "a lone `UnknownBuiltin` on a `main` variable with no registry \
         error is an unrelated blocker, not macro-attributable: {lone:#?}"
    );

    // --- (b) C-LEARN's allowed NON-macro blockers must NOT be flagged ---
    let model_logic_cycle = Diagnostic {
        model: "main".to_string(),
        variable: Some("previous_emissions_intensity_vs_refyr".to_string()),
        error: model_err(ErrorCode::CircularDependency),
        severity: DiagnosticSeverity::Error,
    };
    let dim_mismatch = Diagnostic {
        model: "main".to_string(),
        variable: Some("c_in_mixed_layer".to_string()),
        error: eq(ErrorCode::MismatchedDimensions),
        severity: DiagnosticSeverity::Error,
    };
    // Phase 3's documented limitation: a non-time `$` reference surfaces as
    // an ordinary unresolved-reference diagnostic on a `main` variable.
    let non_time_dollar = Diagnostic {
        model: "main".to_string(),
        variable: Some("\"goal_1.5_for_temperature\"".to_string()),
        error: eq(ErrorCode::DoesNotExist),
        severity: DiagnosticSeverity::Error,
    };
    // A unit-inference WARNING on a macro body (formal-parameter port vars
    // have no units) -- an allowed non-macro unit-error blocker.
    let macro_body_unit_warning = Diagnostic {
        model: macro_model.clone(),
        variable: Some("m".to_string()),
        error: model_err(ErrorCode::UnitMismatch),
        severity: DiagnosticSeverity::Warning,
    };
    let nonmacro = [
        model_logic_cycle,
        dim_mismatch,
        non_time_dollar,
        macro_body_unit_warning,
    ];
    let flagged = macro_attributable_diagnostics(&dm, &nonmacro);
    assert!(
        flagged.is_empty(),
        "C-LEARN's allowed non-macro blockers must NOT be macro-attributable, \
         but the classifier flagged: {flagged:#?}"
    );
}

/// macros.AC6.2 / macros.AC1.7 -- C-LEARN's four macros (`SAMPLE UNTIL`,
/// `SSHAPE`, `RAMP FROM TO`, `INIT`) import as macro-marked models with the
/// correct `MacroSpec`s (including the uninvoked `INIT`, AC1.7), AND the
/// macro registry builds with NO macro-specific errors -- in particular no
/// false `recursive macro: init -> init` from C-LEARN's
/// `:MACRO: INIT(x) ... INIT = INITIAL(x)`.
///
/// HISTORY (#554, FIXED): the MDL importer necessarily renames the Vensim
/// `INITIAL` builtin to `INIT` (`mdl/xmile_compat.rs`; `Expr1` lowering
/// recognizes only the opcode name `init`, not `initial`), so C-LEARN's
/// uninvoked macro stores the datamodel body `init = init(x)`. The recursion
/// detector used to treat that renamed-builtin call as a recursive
/// `init -> init` macro edge and fail the WHOLE `MacroRegistry::build`,
/// which CASCADED: with an empty registry, `SSHAPE`/`SAMPLE UNTIL`/
/// `RAMP FROM TO` stopped shadowing the builtins and their call sites then
/// reported `BadBuiltinArgs`/`UnknownBuiltin`. A single false positive
/// blocked ALL of C-LEARN's macro expansion. #554 fixes this in two
/// coordinated halves sharing `module_functions::is_renamed_opcode_intrinsic`:
/// `collect_called_macros` no longer records the same-named-opcode-intrinsic
/// self-edge, and `BuiltinVisitor::walk` resolves such a call to the
/// intrinsic instead of recursing into the like-named macro. Genuine
/// recursion (`FOO = FOO(x)`, non-intrinsic) is still rejected
/// (macros.AC5.2 unweakened) -- see the `issue_554_*` tests in
/// `src/macro_expansion_tests.rs` and `src/module_functions.rs`.
///
/// This is the C-LEARN macro-expansion regression guard Phase 7 Task 1
/// builds on. It asserts the four macros import correctly AND the
/// #554 macro-attributable cascade is gone (no macro-registry
/// `CircularDependency`). It deliberately does NOT assert that all of
/// C-LEARN compiles -- C-LEARN's non-macro blockers (#552, #553, #363,
/// model-logic deps) remain out of scope -- only that no macro-specific
/// error from #554 fires. `#[ignore]` (C-LEARN is ~53k lines / 1.4 MB;
/// ~4s just to parse).
// Run with: cargo test --release -- --ignored corpus_clearn_macros_import
#[test]
#[ignore]
fn corpus_clearn_macros_import() {
    let path = "../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl";
    let contents =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    let dm = open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));

    // All four C-LEARN macros import as macro-marked models with the
    // correct MacroSpecs (macros.AC1.7: the uninvoked INIT included).
    let expect: &[(&str, &[&str])] = &[
        ("sample_until", &["lasttime", "input", "initval"]),
        ("sshape", &["xin", "profile"]),
        (
            "ramp_from_to",
            &["xfrom", "xto", "tstart", "tend", "islinear"],
        ),
        ("init", &["x"]),
    ];
    for (name, params) in expect {
        let m = dm
            .models
            .iter()
            .find(|m| m.name == *name && m.macro_spec.is_some())
            .unwrap_or_else(|| {
                panic!(
                    "C-LEARN macro {:?} must import as a macro-marked model; \
                     macro models present: {:?}",
                    name,
                    dm.models
                        .iter()
                        .filter(|m| m.macro_spec.is_some())
                        .map(|m| m.name.clone())
                        .collect::<Vec<_>>()
                )
            });
        let spec = m.macro_spec.as_ref().unwrap();
        assert_eq!(
            spec.parameters,
            params.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "C-LEARN macro {:?} parameter list",
            name
        );
        assert_eq!(
            spec.primary_output, *name,
            "C-LEARN macro {:?} primary output is its own name",
            name
        );
        // All four C-LEARN macros are single-output.
        assert!(
            spec.additional_outputs.is_empty(),
            "C-LEARN macro {:?} is single-output",
            name
        );
    }

    // macros.AC6.2: compile C-LEARN via the salsa path, collect every
    // diagnostic, and assert NO diagnostic is *macro-attributable*. C-LEARN's
    // known NON-macro blockers (circular deps, dimension mismatches, unit
    // errors, and a non-time `$` reference -- Phase 3's documented
    // limitation) are expected and explicitly allowed; the assertion is
    // specifically that macro handling itself introduced no error. The
    // classifier (`macro_attributable_diagnostics`, shared with the metasd
    // corpus harness) catches exactly: a project-level macro-registry build
    // error (the #554 cascade class -- a registry failure un-shadows
    // `SSHAPE`/`SAMPLE UNTIL`/`RAMP FROM TO`, turning every call into
    // `BadBuiltinArgs`/`UnknownBuiltin`), an Error-severity diagnostic inside
    // a macro template body, and a macro-resolution-failure error code
    // (`UnknownBuiltin`/`BadBuiltinArgs`/`BadModelName`/`DuplicateMacroName`)
    // on a macro model or project-level. A bare `UnknownDependency` /
    // `DoesNotExist` on a `main` variable (the non-time `$` case, model-logic
    // deps) is NOT macro-attributable -- the classifier deliberately does not
    // mistake it for a macro error.
    let diags = collect_project_diagnostics(&dm);
    let macro_diags = macro_attributable_diagnostics(&dm, &diags);
    assert!(
        macro_diags.is_empty(),
        "macros.AC6.2: C-LEARN must compile with NO macro-attributable \
         diagnostic (its non-macro blockers -- circular deps, dim mismatches, \
         unit errors, the non-time `$` ref -- are out of scope). The #554 fix \
         removed the false `init -> init` macro-registry recursion and its \
         `SSHAPE`/`SAMPLE UNTIL`/`RAMP FROM TO` cascade; this guards that \
         regression. Found {} macro-attributable diagnostic(s): {macro_diags:#?}",
        macro_diags.len()
    );
}
