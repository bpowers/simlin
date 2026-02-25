// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::fs;
use std::io::BufReader;

use simlin_engine::datamodel::{self, Dt, Project, Variable, ViewElement};
use simlin_engine::{mdl, xmile};

// MDL test files that should survive a full MDL -> Project -> MDL -> Project roundtrip
// at the equation/semantic level.
//
// Excluded categories:
//   - macros (:MACRO:) -- the writer rejects them
//   - external data (GET DATA, GET XLS, etc.) -- external file references
//   - ALLOCATE / INVERT MATRIX -- unsupported builtins
//   - models with `inf` in unit ranges -- parser limitation
//   - models with inline lookup definitions -- the writer emits lookup syntax
//     that the parser cannot re-parse (known limitation)
//   - xidz_zidz -- XIDZ 3-arg -> SAFEDIV argument count changes
static TEST_MDL_MODELS: &[&str] = &[
    // simple scalar / math
    "test/test-models/tests/abs/test_abs.mdl",
    "test/test-models/tests/sqrt/test_sqrt.mdl",
    "test/test-models/tests/exp/test_exp.mdl",
    "test/test-models/tests/ln/test_ln.mdl",
    "test/test-models/tests/log/test_log.mdl",
    "test/test-models/tests/power/power.mdl",
    "test/test-models/tests/trig/test_trig.mdl",
    "test/test-models/tests/constant_expressions/test_constant_expressions.mdl",
    "test/test-models/tests/number_handling/test_number_handling.mdl",
    "test/test-models/tests/zeroled_decimals/test_zeroled_decimals.mdl",
    // control flow / logic
    "test/test-models/tests/if_stmt/if_stmt.mdl",
    "test/test-models/tests/logicals/test_logicals.mdl",
    "test/test-models/tests/limits/test_limits.mdl",
    "test/test-models/tests/parentheses/test_parens.mdl",
    // builtins / functions
    "test/test-models/tests/builtin_max/builtin_max.mdl",
    "test/test-models/tests/builtin_min/builtin_min.mdl",
    "test/test-models/tests/function_capitalization/test_function_capitalization.mdl",
    "test/test-models/tests/nested_functions/test_nested_functions.mdl",
    "test/test-models/tests/reference_capitalization/test_reference_capitalization.mdl",
    // stocks / flows / dynamics
    "test/test-models/tests/chained_initialization/test_chained_initialization.mdl",
    "test/test-models/tests/smooth/test_smooth.mdl",
    "test/test-models/tests/smooth_and_stock/test_smooth_and_stock.mdl",
    "test/test-models/tests/active_initial/test_active_initial.mdl",
    "test/test-models/tests/delays/test_delays.mdl",
    "test/test-models/tests/delay_fixed/test_delay_fixed.mdl",
    "test/test-models/tests/trend/test_trend.mdl",
    "test/test-models/tests/forecast/test_forecast.mdl",
    "test/test-models/tests/sample_if_true/test_sample_if_true.mdl",
    "test/test-models/tests/input_functions/test_inputs.mdl",
    "test/test-models/tests/game/test_game.mdl",
    "test/test-models/tests/time/test_time.mdl",
    "test/test-models/tests/euler_step_vs_saveper/test_euler_step_vs_saveper.mdl",
    // formatting / whitespace
    "test/test-models/tests/line_breaks/test_line_breaks.mdl",
    "test/test-models/tests/line_continuation/test_line_continuation.mdl",
    "test/test-models/tests/model_doc/model_doc.mdl",
    // sample models
    "test/test-models/samples/SIR/SIR.mdl",
    "test/test-models/samples/Lotka_Volterra/Lotka_Volterra.mdl",
    "test/test-models/samples/simple_harmonic_oscillator/simple_harmonic_oscillator.mdl",
    "test/test-models/samples/Roessler_Chaos/roessler_chaos.mdl",
    // sdeverywhere models
    "test/sdeverywhere/models/smooth/smooth.mdl",
    "test/sdeverywhere/models/smooth3/smooth3.mdl",
    "test/sdeverywhere/models/delay/delay.mdl",
    "test/sdeverywhere/models/sir/sir.mdl",
    "test/sdeverywhere/models/active_initial/active_initial.mdl",
    "test/sdeverywhere/models/initial/initial.mdl",
    "test/sdeverywhere/models/trend/trend.mdl",
    "test/sdeverywhere/models/sample/sample.mdl",
    "test/sdeverywhere/models/pulsetrain/pulsetrain.mdl",
    "test/sdeverywhere/models/quantum/quantum.mdl",
    "test/sdeverywhere/models/npv/npv.mdl",
    "test/sdeverywhere/models/comments/comments.mdl",
];

// XMILE/STMX files for cross-format testing (XMILE -> MDL -> re-parse).
static TEST_XMILE_MODELS: &[&str] = &[
    "test/test-models/tests/abs/test_abs.xmile",
    "test/test-models/tests/sqrt/test_sqrt.xmile",
    "test/test-models/tests/exp/test_exp.xmile",
    "test/test-models/tests/ln/test_ln.xmile",
    "test/test-models/tests/log/test_log.xmile",
    "test/test-models/tests/constant_expressions/test_constant_expressions.xmile",
    "test/test-models/tests/if_stmt/if_stmt.xmile",
    "test/test-models/tests/logicals/test_logicals.xmile",
    "test/test-models/tests/builtin_max/builtin_max.xmile",
    "test/test-models/tests/builtin_min/builtin_min.xmile",
    "test/test-models/tests/parentheses/test_parens.xmile",
    "test/test-models/tests/limits/test_limits.xmile",
    "test/test-models/tests/number_handling/test_number_handling.xmile",
    "test/test-models/tests/function_capitalization/test_function_capitalization.xmile",
    "test/test-models/tests/chained_initialization/test_chained_initialization.xmile",
    "test/test-models/tests/line_breaks/test_line_breaks.xmile",
    "test/test-models/tests/reference_capitalization/test_reference_capitalization.xmile",
    "test/test-models/samples/teacup/teacup.xmile",
];

// MDL files with non-trivial sketch sections for view roundtrip testing.
// The sketch roundtrip does not yet preserve stock/flow element types,
// so this tests that variable names and link counts survive.
static TEST_MDL_MODELS_WITH_SKETCH: &[&str] = &[
    "test/test-models/tests/if_stmt/if_stmt.mdl",
    "test/test-models/tests/logicals/test_logicals.mdl",
    "test/test-models/tests/constant_expressions/test_constant_expressions.mdl",
    "test/test-models/tests/parentheses/test_parens.mdl",
    "test/test-models/tests/number_handling/test_number_handling.mdl",
];

fn resolve_path(relative: &str) -> String {
    format!("../../{relative}")
}

/// Get the equation text from a Variable regardless of its type.
fn var_equation_text(var: &Variable) -> &str {
    match var.get_equation() {
        Some(datamodel::Equation::Scalar(s)) => s.as_str(),
        Some(datamodel::Equation::ApplyToAll(_, s)) => s.as_str(),
        Some(datamodel::Equation::Arrayed(_, _)) => "<arrayed>",
        None => "",
    }
}

/// Compare two Projects for semantic equivalence: same variable names and
/// equation text, same sim specs, same dimensions. Ignores view layout and
/// variable type (Stock vs Aux vs Flow) since the sketch roundtrip does not
/// yet preserve element types. Also ignores `source`.
fn assert_semantic_equivalence(a: &Project, b: &Project, path: &str) -> Option<String> {
    if a.sim_specs != b.sim_specs {
        return Some(format!("{path}: sim_specs differ"));
    }

    if a.dimensions.len() != b.dimensions.len() {
        return Some(format!(
            "{path}: dimension count differs: {} vs {}",
            a.dimensions.len(),
            b.dimensions.len()
        ));
    }
    for (da, db) in a.dimensions.iter().zip(&b.dimensions) {
        if da != db {
            return Some(format!("{path}: dimension {:?} differs", da.name));
        }
    }

    if a.models.len() != b.models.len() {
        return Some(format!(
            "{path}: model count differs: {} vs {}",
            a.models.len(),
            b.models.len()
        ));
    }

    for (i, (ma, mb)) in a.models.iter().zip(&b.models).enumerate() {
        if let Some(msg) = assert_model_equivalence(ma, mb, path, i) {
            return Some(msg);
        }
    }

    None
}

fn assert_model_equivalence(
    ma: &datamodel::Model,
    mb: &datamodel::Model,
    path: &str,
    i: usize,
) -> Option<String> {
    if ma.variables.len() != mb.variables.len() {
        return Some(format!(
            "{path}: model[{i}] variable count: {} vs {}",
            ma.variables.len(),
            mb.variables.len()
        ));
    }

    let mut vars_a: Vec<_> = ma.variables.iter().collect();
    let mut vars_b: Vec<_> = mb.variables.iter().collect();
    vars_a.sort_by_key(|v| v.get_ident().to_owned());
    vars_b.sort_by_key(|v| v.get_ident().to_owned());

    for (va, vb) in vars_a.iter().zip(&vars_b) {
        if va.get_ident() != vb.get_ident() {
            return Some(format!(
                "{path}: model[{i}] variable name mismatch: {:?} vs {:?}",
                va.get_ident(),
                vb.get_ident()
            ));
        }

        let eq_a = var_equation_text(va);
        let eq_b = var_equation_text(vb);
        if eq_a != eq_b {
            return Some(format!(
                "{path}: model[{i}] var {:?} equation differs: {:?} vs {:?}",
                va.get_ident(),
                eq_a,
                eq_b
            ));
        }
    }

    None
}

/// Compare sim specs loosely: XMILE stores save_step as explicit Option<Dt>
/// while MDL infers it from the SAVEPER equation. We compare the numeric
/// values only.
fn sim_specs_equivalent(a: &datamodel::SimSpecs, b: &datamodel::SimSpecs) -> bool {
    if (a.start - b.start).abs() > f64::EPSILON {
        return false;
    }
    if (a.stop - b.stop).abs() > f64::EPSILON {
        return false;
    }
    // Compare dt values
    let dt_a = match &a.dt {
        Dt::Dt(v) => *v,
        Dt::Reciprocal(v) => 1.0 / *v,
    };
    let dt_b = match &b.dt {
        Dt::Dt(v) => *v,
        Dt::Reciprocal(v) => 1.0 / *v,
    };
    if (dt_a - dt_b).abs() > 1e-10 {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// Task 1: MDL -> MDL roundtrip
// ---------------------------------------------------------------------------

#[test]
fn mdl_to_mdl_roundtrip() {
    let mut failures: Vec<String> = Vec::new();

    for &path in TEST_MDL_MODELS {
        let file_path = resolve_path(path);
        let source = match fs::read_to_string(&file_path) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{path}: read error: {e}"));
                continue;
            }
        };

        let project1 = match mdl::parse_mdl(&source) {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{path}: initial parse error: {e}"));
                continue;
            }
        };

        let mdl_text = match mdl::project_to_mdl(&project1) {
            Ok(t) => t,
            Err(e) => {
                failures.push(format!("{path}: write error: {e}"));
                continue;
            }
        };

        let project2 = match mdl::parse_mdl(&mdl_text) {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{path}: re-parse error: {e}"));
                continue;
            }
        };

        if let Some(msg) = assert_semantic_equivalence(&project1, &project2, path) {
            failures.push(msg);
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} MDL roundtrip failure(s):\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}

// ---------------------------------------------------------------------------
// Task 2: XMILE -> MDL cross-format roundtrip
// ---------------------------------------------------------------------------

/// Canonicalize a variable name for cross-format comparison.
/// XMILE uses spaces and mixed case ("FlowA", "My Variable") while
/// MDL uses underscores and lowercase ("flowa", "my_variable").
fn canonical_name(name: &str) -> String {
    name.replace(' ', "_").to_lowercase()
}

/// Compare two Projects for cross-format semantic equivalence.
/// More lenient than same-format comparison: ignores sim spec details
/// (save_step representation), name casing/spacing, and compares only
/// core model structure.
fn assert_cross_format_equivalence(
    xmile_proj: &Project,
    mdl_proj: &Project,
    path: &str,
) -> Option<String> {
    if !sim_specs_equivalent(&xmile_proj.sim_specs, &mdl_proj.sim_specs) {
        return Some(format!("{path}: sim_specs not numerically equivalent"));
    }

    if xmile_proj.models.len() != mdl_proj.models.len() {
        return Some(format!(
            "{path}: model count: {} vs {}",
            xmile_proj.models.len(),
            mdl_proj.models.len()
        ));
    }

    for (i, (ma, mb)) in xmile_proj.models.iter().zip(&mdl_proj.models).enumerate() {
        if ma.variables.len() != mb.variables.len() {
            return Some(format!(
                "{path}: model[{i}] variable count: {} vs {}",
                ma.variables.len(),
                mb.variables.len()
            ));
        }

        let mut names_a: Vec<_> = ma
            .variables
            .iter()
            .map(|v| canonical_name(v.get_ident()))
            .collect();
        let mut names_b: Vec<_> = mb
            .variables
            .iter()
            .map(|v| canonical_name(v.get_ident()))
            .collect();
        names_a.sort();
        names_b.sort();

        for (na, nb) in names_a.iter().zip(&names_b) {
            if na != nb {
                return Some(format!(
                    "{path}: model[{i}] canonical name mismatch: {:?} vs {:?}",
                    na, nb
                ));
            }
        }
    }

    None
}

#[test]
fn xmile_to_mdl_roundtrip() {
    let mut failures: Vec<String> = Vec::new();

    for &path in TEST_XMILE_MODELS {
        let file_path = resolve_path(path);
        let source = match fs::read_to_string(&file_path) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{path}: read error: {e}"));
                continue;
            }
        };

        let mut reader = BufReader::new(source.as_bytes());
        let xmile_project = match xmile::project_from_reader(&mut reader) {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{path}: XMILE parse error: {e}"));
                continue;
            }
        };

        if xmile_project.models.len() != 1 {
            continue;
        }
        let has_modules = xmile_project.models[0]
            .variables
            .iter()
            .any(|v| matches!(v, Variable::Module(_)));
        if has_modules {
            continue;
        }

        let mdl_text = match mdl::project_to_mdl(&xmile_project) {
            Ok(t) => t,
            Err(e) => {
                failures.push(format!("{path}: MDL write error: {e}"));
                continue;
            }
        };

        let mdl_project = match mdl::parse_mdl(&mdl_text) {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{path}: MDL re-parse error: {e}"));
                continue;
            }
        };

        if let Some(msg) = assert_cross_format_equivalence(&xmile_project, &mdl_project, path) {
            failures.push(msg);
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} XMILE->MDL roundtrip failure(s):\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}

// ---------------------------------------------------------------------------
// Task 3: View/sketch roundtrip
// ---------------------------------------------------------------------------

/// Count view elements by type.
fn count_view_elements(view: &datamodel::View) -> (usize, usize, usize, usize, usize, usize) {
    let elements = match view {
        datamodel::View::StockFlow(sf) => &sf.elements,
    };
    let (mut stocks, mut flows, mut auxes, mut links, mut clouds, mut aliases) = (0, 0, 0, 0, 0, 0);
    for el in elements {
        match el {
            ViewElement::Stock(_) => stocks += 1,
            ViewElement::Flow(_) => flows += 1,
            ViewElement::Aux(_) => auxes += 1,
            ViewElement::Link(_) => links += 1,
            ViewElement::Cloud(_) => clouds += 1,
            ViewElement::Alias(_) => aliases += 1,
            ViewElement::Module(_) | ViewElement::Group(_) => {}
        }
    }
    (stocks, flows, auxes, links, clouds, aliases)
}

/// Collect the set of variable names referenced in a view.
fn collect_view_names(view: &datamodel::View) -> Vec<String> {
    let elements = match view {
        datamodel::View::StockFlow(sf) => &sf.elements,
    };
    let mut names: Vec<String> = elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Aux(a) => Some(a.name.clone()),
            ViewElement::Stock(s) => Some(s.name.clone()),
            ViewElement::Flow(f) => Some(f.name.clone()),
            _ => None,
        })
        .collect();
    names.sort();
    names
}

#[test]
fn view_element_roundtrip() {
    let mut failures: Vec<String> = Vec::new();

    for &path in TEST_MDL_MODELS_WITH_SKETCH {
        let file_path = resolve_path(path);
        let source = match fs::read_to_string(&file_path) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{path}: read error: {e}"));
                continue;
            }
        };

        let project1 = match mdl::parse_mdl(&source) {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{path}: initial parse error: {e}"));
                continue;
            }
        };

        let mdl_text = match mdl::project_to_mdl(&project1) {
            Ok(t) => t,
            Err(e) => {
                failures.push(format!("{path}: write error: {e}"));
                continue;
            }
        };

        let project2 = match mdl::parse_mdl(&mdl_text) {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{path}: re-parse error: {e}"));
                continue;
            }
        };

        for (i, (m1, m2)) in project1.models.iter().zip(&project2.models).enumerate() {
            if m1.views.len() != m2.views.len() {
                failures.push(format!(
                    "{path}: model[{i}] view count differs: {} vs {}",
                    m1.views.len(),
                    m2.views.len()
                ));
                continue;
            }

            for (j, (v1, v2)) in m1.views.iter().zip(&m2.views).enumerate() {
                let names1 = collect_view_names(v1);
                let names2 = collect_view_names(v2);
                if names1 != names2 {
                    failures.push(format!(
                        "{path}: model[{i}].view[{j}] variable names differ: {names1:?} vs {names2:?}"
                    ));
                }

                let counts1 = count_view_elements(v1);
                let counts2 = count_view_elements(v2);
                if counts1 != counts2 {
                    let (stocks1, flows1, auxes1, links1, clouds1, aliases1) = counts1;
                    let (stocks2, flows2, auxes2, links2, clouds2, aliases2) = counts2;
                    failures.push(format!(
                        "{path}: model[{i}].view[{j}] element counts differ: \
                         stocks={stocks1}vs{stocks2} flows={flows1}vs{flows2} \
                         auxes={auxes1}vs{auxes2} links={links1}vs{links2} \
                         clouds={clouds1}vs{clouds2} aliases={aliases1}vs{aliases2}"
                    ));
                }
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} view roundtrip failure(s):\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}

// ---------------------------------------------------------------------------
// Ignored test: produces files for manual Vensim validation
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn write_mdl_for_vensim_validation() {
    let output_dir = std::env::temp_dir().join("simlin-mdl-roundtrip");
    fs::create_dir_all(&output_dir).expect("create output dir");

    let models_to_export: &[&str] = &[
        "test/test-models/tests/abs/test_abs.mdl",
        "test/test-models/tests/smooth/test_smooth.mdl",
        "test/test-models/tests/forecast/test_forecast.mdl",
        "test/test-models/samples/SIR/SIR.mdl",
    ];

    for &path in models_to_export {
        let file_path = resolve_path(path);
        let source = fs::read_to_string(&file_path).unwrap_or_else(|e| panic!("read {path}: {e}"));

        let project = mdl::parse_mdl(&source).unwrap_or_else(|e| panic!("parse {path}: {e}"));

        let mdl_text =
            mdl::project_to_mdl(&project).unwrap_or_else(|e| panic!("write {path}: {e}"));

        let filename = std::path::Path::new(path)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        let output_path = output_dir.join(format!("roundtrip_{filename}"));
        fs::write(&output_path, &mdl_text)
            .unwrap_or_else(|e| panic!("write output {}: {e}", output_path.display()));

        eprintln!("Wrote: {}", output_path.display());
    }

    eprintln!(
        "\nOutput directory: {}\nOpen these files in Vensim to verify diagrams and simulation.",
        output_dir.display()
    );
}
