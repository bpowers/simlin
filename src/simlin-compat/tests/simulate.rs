// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::rc::Rc;

use float_cmp::approx_eq;

use simlin_compat::{load_csv, load_dat, xmile};
use simlin_engine::common::{Canonical, Ident};
use simlin_engine::interpreter::Simulation;
use simlin_engine::serde::{deserialize, serialize};
use simlin_engine::{Project, Results, Vm, project_io};

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
    "test/test-models/tests/lookups/test_lookups.xmile",
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
        use simlin_compat::prost::Message;

        let pb_project_inner = serialize(&datamodel_project);
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

#[test]
#[ignore]
fn simulates_except() {
    simulate_path("../../test/sdeverywhere/models/except/except.xmile");
}

#[test]
#[ignore]
fn simulates_sum() {
    simulate_path("../../test/sdeverywhere/models/sum/sum.xmile");
}

// Ignored: The sum model contains cross-dimension broadcasting expressions like
// "SUM(a[*]+h[*])" where a[DimA] and h[DimC] have different dimensions. The expected
// behavior is a 3x3 cross-product (9 elements summed = 198), but the interpreter
// currently treats this as element-wise (3 elements summed = 66).
// This is a separate interpreter limitation, not related to dimension-name subscripts.
#[test]
#[ignore]
fn simulates_sum_interpreter_only() {
    simulate_path_interpreter_only("../../test/sdeverywhere/models/sum/sum.xmile");
}

// Ignored: The except model uses Vensim subscript mappings (e.g., "DimD: D1, D2 -> (DimA: SubA, A1)")
// that are not preserved in the XMILE conversion. Without these mappings, equations like
// "k[DimA] = a[DimA] + j[DimD]" cannot be resolved since there's no way to map DimD elements
// to DimA elements.
#[test]
#[ignore]
fn simulates_except_interpreter_only() {
    simulate_path_interpreter_only("../../test/sdeverywhere/models/except/except.xmile");
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
    "test/sdeverywhere/models/smooth3/smooth3.xmile",
    "test/sdeverywhere/models/specialchars/specialchars.xmile",
    "test/sdeverywhere/models/subalias/subalias.xmile",
    "test/sdeverywhere/models/trend/trend.xmile",
    //
    // Failing tests - commented out with reasons
    //
    // NotSimulatable: uses ALLOCATE AVAILABLE builtin
    // "test/sdeverywhere/models/allocate/allocate.xmile",
    //
    // NotSimulatable: uses GET DIRECT CONSTANTS
    // "test/sdeverywhere/models/arrays_cname/arrays_cname.xmile",
    // "test/sdeverywhere/models/arrays_varname/arrays_varname.xmile",
    //
    // EmptyEquation: uses DELAY FIXED builtin
    // "test/sdeverywhere/models/delayfixed/delayfixed.xmile",
    // "test/sdeverywhere/models/delayfixed2/delayfixed2.xmile",
    //
    // Wrong values: sparse array initialization not fully supported
    // "test/sdeverywhere/models/directconst/directconst.xmile",
    //
    // Assertion failure in dat loading: time ordering issue
    // "test/sdeverywhere/models/directdata/directdata.xmile",
    //
    // EmptyEquation: uses GET DIRECT LOOKUPS
    // "test/sdeverywhere/models/directlookups/directlookups.xmile",
    //
    // EmptyEquation: uses GET DIRECT SUBSCRIPT
    // "test/sdeverywhere/models/directsubs/directsubs.xmile",
    //
    // MismatchedDimensions: uses Vensim EXCEPT syntax with subscript mappings
    // "test/sdeverywhere/models/except/except.xmile",
    // "test/sdeverywhere/models/except2/except2.xmile",
    //
    // EmptyEquation: uses GET XLS DATA
    // "test/sdeverywhere/models/extdata/extdata.xmile",
    //
    // No expected results / not test models (preprocessing test files)
    // "test/sdeverywhere/models/flatten/expected.xmile",
    // "test/sdeverywhere/models/flatten/input1.xmile",
    // "test/sdeverywhere/models/flatten/input2.xmile",
    //
    // EmptyEquation: uses GET DATA BETWEEN TIMES
    // "test/sdeverywhere/models/getdata/getdata.xmile",
    //
    // EmptyEquation: uses INTEG with complex initialization
    // "test/sdeverywhere/models/interleaved/interleaved.xmile",
    //
    // EmptyEquation: uses :EXCEPT: in equations
    // "test/sdeverywhere/models/longeqns/longeqns.xmile",
    //
    // MismatchedDimensions: uses subscript mappings
    // "test/sdeverywhere/models/mapping/mapping.xmile",
    // "test/sdeverywhere/models/multimap/multimap.xmile",
    //
    // EmptyEquation: uses NPV builtin
    // "test/sdeverywhere/models/npv/npv.xmile",
    //
    // No expected results (preprocessing test files)
    // "test/sdeverywhere/models/preprocess/expected.xmile",
    // "test/sdeverywhere/models/preprocess/input.xmile",
    //
    // No expected results
    // "test/sdeverywhere/models/prune/prune.xmile",
    //
    // EmptyEquation: uses QUANTUM builtin
    // "test/sdeverywhere/models/quantum/quantum.xmile",
    //
    // EmptyEquation: uses :EXCEPT: syntax
    // "test/sdeverywhere/models/ref/ref.xmile",
    //
    // EmptyEquation: uses SAMPLE IF TRUE builtin
    // "test/sdeverywhere/models/sample/sample.xmile",
    //
    // No expected results (nested model directory)
    // "test/sdeverywhere/models/sir/model/sir.xmile",
    //
    // Values don't match: SMOOTH implementation difference
    // "test/sdeverywhere/models/smooth/smooth.xmile",
    //
    // MismatchedDimensions: uses subscript mappings
    // "test/sdeverywhere/models/subscript/subscript.xmile",
    //
    // Cross-dimension broadcasting: SUM(a[*]+h[*]) with different dimensions
    // "test/sdeverywhere/models/sum/sum.xmile",
    //
    // EmptyEquation: uses SUM OF builtin with condition
    // "test/sdeverywhere/models/sumif/sumif.xmile",
    //
    // MismatchedDimensions: uses VECTOR ELM MAP
    // "test/sdeverywhere/models/vector/vector.xmile",
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
