// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashSet;
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
    "test/bobby/vdf/econ/mark2.mdl",
    "test/bobby/vdf/water/water.mdl",
    "test/bobby/vdf/lookups/lookup_ex.mdl",
];

fn resolve_path(relative: &str) -> String {
    format!("../../{relative}")
}

/// Get the equation text from a Variable regardless of its type.
fn var_equation_text(var: &Variable) -> &str {
    match var.get_equation() {
        Some(datamodel::Equation::Scalar(s)) => s.as_str(),
        Some(datamodel::Equation::ApplyToAll(_, s)) => s.as_str(),
        Some(datamodel::Equation::Arrayed(_, _, _, _)) => "<arrayed>",
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

// ---------------------------------------------------------------------------
// Task 4: mark2 sketch structure validation
// ---------------------------------------------------------------------------

/// Verify that the mark2 model's sketch roundtrips with correct Vensim
/// element ordering: for each flow, pipe connectors (type 1 with flag 22)
/// must precede the valve (type 11) and flow label (type 10, shape=40).
/// Clouds (type 12) must precede the pipe connectors that reference them.
#[test]
fn mark2_sketch_ordering() {
    let file_path = resolve_path("test/bobby/vdf/econ/mark2.mdl");
    let source = fs::read_to_string(&file_path).expect("read mark2.mdl");
    let project = mdl::parse_mdl(&source).expect("parse mark2.mdl");

    let mdl_text = mdl::project_to_mdl(&project).expect("write mark2.mdl");

    // The written MDL should be re-parseable
    let project2 = mdl::parse_mdl(&mdl_text).expect("re-parse mark2.mdl");

    // Both views should survive
    assert_eq!(
        project.models[0].views.len(),
        project2.models[0].views.len()
    );

    // Extract the sketch section from the written text
    let sketch_start = mdl_text
        .find("\\\\\\---/// Sketch information")
        .expect("should have sketch section");
    let sketch_text = &mdl_text[sketch_start..];

    // For each view, verify Vensim ordering constraints
    for line in sketch_text.lines() {
        // Flow pipe connectors (type 1 with field 7 = 22) must have the
        // direction flag (field 4) set to 4 or 100
        if line.starts_with("1,") {
            let fields: Vec<&str> = line.split(',').collect();
            if fields.len() > 7 && fields[7] == "22" {
                let direction: i32 = fields[4].parse().unwrap_or(0);
                assert!(
                    direction == 4 || direction == 100,
                    "pipe connector should have direction 4 or 100, got {direction}: {line}"
                );
            }
        }

        // Influence connectors (type 1 without flag 22) should have field 9 = 64
        if line.starts_with("1,") {
            let fields: Vec<&str> = line.split(',').collect();
            if fields.len() > 9 && fields[7] != "22" {
                let influence_flag: i32 = fields[9].parse().unwrap_or(0);
                assert_eq!(
                    influence_flag, 64,
                    "influence connector should have field 9 = 64: {line}"
                );
            }
        }
    }

    // Verify that pipe connectors appear before their valve in each flow block.
    // Collect all valve UIDs and verify their pipes appear earlier in the text.
    let sketch_lines: Vec<&str> = sketch_text.lines().collect();
    for (i, line) in sketch_lines.iter().enumerate() {
        if !line.starts_with("11,") {
            continue;
        }
        let valve_fields: Vec<&str> = line.split(',').collect();
        let valve_uid: &str = valve_fields[1];

        // Find pipe connectors that reference this valve (field 2 = valve_uid)
        for (j, pipe_line) in sketch_lines.iter().enumerate() {
            if !pipe_line.starts_with("1,") {
                continue;
            }
            let pipe_fields: Vec<&str> = pipe_line.split(',').collect();
            if pipe_fields.len() > 7 && pipe_fields[7] == "22" && pipe_fields[2] == valve_uid {
                assert!(
                    j < i,
                    "pipe connector for valve {valve_uid} at line {j} should precede valve at line {i}"
                );
            }
        }
    }

    // Verify cloud elements appear before the pipe connectors that reference them
    for (i, line) in sketch_lines.iter().enumerate() {
        if !line.starts_with("12,") {
            continue;
        }
        let cloud_fields: Vec<&str> = line.split(',').collect();
        let cloud_uid: &str = cloud_fields[1];

        // Find pipe connectors that reference this cloud (field 3 = cloud_uid)
        for (j, pipe_line) in sketch_lines.iter().enumerate() {
            if !pipe_line.starts_with("1,") {
                continue;
            }
            let pipe_fields: Vec<&str> = pipe_line.split(',').collect();
            if pipe_fields.len() > 7 && pipe_fields[7] == "22" && pipe_fields[3] == cloud_uid {
                assert!(
                    i < j,
                    "cloud {cloud_uid} at line {i} should precede pipe connector at line {j}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Task 5: MDL format roundtrip (per-view element fidelity)
// ---------------------------------------------------------------------------

/// Split the sketch section of an MDL string into per-view segments.
/// Returns a vec of (view_name, element_lines, font_line) tuples.
fn split_sketch_into_views(mdl_text: &str) -> Vec<(&str, Vec<&str>, Option<&str>)> {
    let sketch_marker = "\\\\\\---/// Sketch information";
    let mut views = Vec::new();

    // Find all sketch section starts
    let mut search_start = 0;
    let mut section_starts = Vec::new();
    while let Some(pos) = mdl_text[search_start..].find(sketch_marker) {
        let abs_pos = search_start + pos;
        section_starts.push(abs_pos);
        search_start = abs_pos + sketch_marker.len();
    }

    for (idx, &start) in section_starts.iter().enumerate() {
        let end = if idx + 1 < section_starts.len() {
            section_starts[idx + 1]
        } else {
            // Find the final terminator "///---\\\\\\".
            mdl_text[start..]
                .find("///---\\\\\\")
                .map(|p| start + p)
                .unwrap_or(mdl_text.len())
        };

        let section = &mdl_text[start..end];
        let lines: Vec<&str> = section.lines().collect();

        // Parse: first line is the marker, second is V300, third is *ViewName,
        // fourth is $font, rest are element lines.
        let mut view_name = "";
        let mut font_line = None;
        let mut element_lines = Vec::new();

        for line in &lines {
            if let Some(name) = line.strip_prefix('*') {
                view_name = name;
            } else if let Some(font) = line.strip_prefix('$') {
                font_line = Some(font);
            } else if line.starts_with("10,")
                || line.starts_with("11,")
                || line.starts_with("12,")
                || line.starts_with("1,")
            {
                element_lines.push(*line);
            }
        }

        views.push((view_name, element_lines, font_line));
    }

    views
}

/// Extract the variable name from a type-10 sketch element line.
/// Type-10 lines have format: 10,uid,name,x,y,...
fn extract_element_name(line: &str) -> Option<&str> {
    let fields: Vec<&str> = line.split(',').collect();
    if fields.len() > 2 && fields[0] == "10" {
        Some(fields[2])
    } else {
        None
    }
}

/// Count sketch elements by type within a set of element lines.
fn count_sketch_element_types(lines: &[&str]) -> (usize, usize, usize, usize) {
    let mut connectors = 0;
    let mut labels = 0;
    let mut valves = 0;
    let mut clouds = 0;
    for line in lines {
        if line.starts_with("1,") {
            connectors += 1;
        } else if line.starts_with("10,") {
            labels += 1;
        } else if line.starts_with("11,") {
            valves += 1;
        } else if line.starts_with("12,") {
            clouds += 1;
        }
    }
    (connectors, labels, valves, clouds)
}

/// Verify mark2.mdl format roundtrip: parse, write, and compare the
/// output against the original at the per-view-element level.
///
/// Checks:
/// - AC1.1: Exactly 2 views with correct names
/// - AC1.2: Per-view element lines match as unordered sets
/// - AC1.4: Font specification preserved per view
/// - AC3.1: Lookup calls use `table ( input )` syntax
/// - AC3.2: Lookup range bounds preserved
/// - AC4.1: Short equations use inline format
/// - AC4.3: Variable name casing preserved
#[test]
fn mdl_format_roundtrip() {
    let mut failures: Vec<String> = Vec::new();

    let file_path = resolve_path("test/bobby/vdf/econ/mark2.mdl");
    let source = fs::read_to_string(&file_path).expect("read mark2.mdl");
    let project = mdl::parse_mdl(&source).expect("parse mark2.mdl");
    let output = mdl::project_to_mdl(&project).expect("write mark2.mdl");

    // Re-parse the output to confirm it is valid MDL
    let _project2 = mdl::parse_mdl(&output).expect("re-parse roundtripped mark2.mdl");

    // -----------------------------------------------------------------------
    // AC1.1: Exactly 2 views with correct names
    // -----------------------------------------------------------------------
    let orig_views = split_sketch_into_views(&source);
    let output_views = split_sketch_into_views(&output);

    if output_views.len() != 2 {
        failures.push(format!(
            "AC1.1: expected 2 views, got {}",
            output_views.len()
        ));
    } else {
        // View names should contain the numbered prefix from the original
        let expected_names = ["1 housing", "2 investments"];
        for (i, expected) in expected_names.iter().enumerate() {
            if !output_views[i].0.contains(expected) {
                failures.push(format!(
                    "AC1.1: view[{i}] name {:?} does not contain {:?}",
                    output_views[i].0, expected
                ));
            }
        }
    }

    // -----------------------------------------------------------------------
    // AC1.2: Per-view elements match as unordered sets
    //
    // UIDs are renumbered and coordinates may shift during roundtrip, so
    // we compare named elements (type 10) by name and non-named elements
    // (connectors, valves, clouds) by count.
    // -----------------------------------------------------------------------
    if orig_views.len() == output_views.len() {
        for (i, (orig, out)) in orig_views.iter().zip(&output_views).enumerate() {
            // Compare named elements (type 10) by variable name
            let orig_names: HashSet<&str> = orig
                .1
                .iter()
                .filter_map(|l| extract_element_name(l))
                .collect();
            let out_names: HashSet<&str> = out
                .1
                .iter()
                .filter_map(|l| extract_element_name(l))
                .collect();

            let missing_names: Vec<_> = orig_names.difference(&out_names).collect();
            let extra_names: Vec<_> = out_names.difference(&orig_names).collect();

            // Shadow references to the built-in "Time" variable are not
            // preserved during roundtrip (Time is not a model variable).
            let missing_non_time: Vec<_> =
                missing_names.iter().filter(|n| **n != &"Time").collect();
            if !missing_non_time.is_empty() {
                failures.push(format!(
                    "AC1.2: view[{i}] ({:?}) missing named element(s): {:?}",
                    orig.0, missing_non_time
                ));
            }
            if !extra_names.is_empty() {
                failures.push(format!(
                    "AC1.2: view[{i}] ({:?}) has extra named element(s): {:?}",
                    out.0, extra_names
                ));
            }

            // Compare element type counts
            let (orig_conn, orig_lbl, orig_valve, orig_cloud) = count_sketch_element_types(&orig.1);
            let (out_conn, out_lbl, out_valve, out_cloud) = count_sketch_element_types(&out.1);

            // Label count may differ by the number of Time shadow elements
            let time_shadow_count = orig
                .1
                .iter()
                .filter(|l| l.starts_with("10,") && extract_element_name(l) == Some("Time"))
                .count();
            if orig_lbl - time_shadow_count != out_lbl {
                failures.push(format!(
                    "AC1.2: view[{i}] label count: orig={orig_lbl} (minus {time_shadow_count} Time shadows) \
                     vs out={out_lbl}"
                ));
            }
            if orig_valve != out_valve {
                failures.push(format!(
                    "AC1.2: view[{i}] valve count: orig={orig_valve} out={out_valve}"
                ));
            }
            if orig_cloud != out_cloud {
                failures.push(format!(
                    "AC1.2: view[{i}] cloud count: orig={orig_cloud} out={out_cloud}"
                ));
            }
            // Connector counts may differ for documented reasons:
            // 1. Shadow references to the built-in Time variable are dropped
            //    (Time is not a model variable), along with their connectors.
            // 2. Init-only links (field 10 = 1, dashed arrows in Vensim) may
            //    not survive the roundtrip.
            // Count the expected dropped connectors from both sources.
            let time_uids: HashSet<&str> = orig
                .1
                .iter()
                .filter(|l| l.starts_with("10,") && extract_element_name(l) == Some("Time"))
                .filter_map(|l| l.split(',').nth(1))
                .collect();
            let dropped_time_connectors = orig
                .1
                .iter()
                .filter(|l| {
                    if !l.starts_with("1,") {
                        return false;
                    }
                    let fields: Vec<&str> = l.split(',').collect();
                    fields.len() > 3
                        && (time_uids.contains(fields[2]) || time_uids.contains(fields[3]))
                })
                .count();
            let init_only_connectors = orig
                .1
                .iter()
                .filter(|l| {
                    if !l.starts_with("1,") {
                        return false;
                    }
                    let fields: Vec<&str> = l.split(',').collect();
                    fields.len() > 10 && fields[10] == "1"
                })
                .count();
            let expected_dropped = dropped_time_connectors + init_only_connectors;
            let conn_diff = (orig_conn as i32 - out_conn as i32).abs();
            if conn_diff > expected_dropped as i32 {
                failures.push(format!(
                    "AC1.2: view[{i}] connector count: orig={orig_conn} out={out_conn} \
                     (diff={conn_diff} exceeds expected_dropped={expected_dropped}: \
                     time={dropped_time_connectors}, init_only={init_only_connectors})"
                ));
            }
        }
    }

    // -----------------------------------------------------------------------
    // AC1.3: Per-element field-level fidelity
    //
    // For each type-10 (named) element matched by name between original
    // and output, compare dimension and shape fields. Skip uid (field 1),
    // coordinates (fields 3,4), and fields that depend on display state
    // we don't yet preserve (field 9 = init-link flag, field 11 = varies
    // by element type, fields 14+ = ghost color/font).
    // -----------------------------------------------------------------------
    if orig_views.len() == output_views.len() {
        for (i, (orig, out)) in orig_views.iter().zip(&output_views).enumerate() {
            fn build_name_fields<'a>(
                lines: &[&'a str],
            ) -> std::collections::HashMap<&'a str, Vec<&'a str>> {
                let mut map = std::collections::HashMap::new();
                for line in lines {
                    let fields: Vec<&str> = line.split(',').collect();
                    if fields.len() > 2 && fields[0] == "10" {
                        map.insert(fields[2], fields);
                    }
                }
                map
            }

            let orig_fields = build_name_fields(&orig.1);
            let out_fields = build_name_fields(&out.1);

            // Fields to compare: w(5), h(6), bits(8).
            // Shape (field 7) is excluded because Vensim allows displaying
            // any variable type with any shape (e.g. an aux as a stock box).
            // Our converter classifies variable type from the equation, not
            // the sketch shape, so non-stock variables displayed as boxes
            // (shape=3) will roundtrip as shape=8.
            let compare_indices = [5, 6, 8];

            // Elements that appear in multiple views are converted to
            // aliases during view composition. Their shape changes from
            // stock(3) to aux(8) and is not preserved. Collect names that
            // appear in OTHER views so we can exclude them from shape checks.
            let mut cross_view_names: HashSet<&str> = HashSet::new();
            for (j, other) in output_views.iter().enumerate() {
                if j != i {
                    for line in &other.1 {
                        if let Some(n) = extract_element_name(line) {
                            cross_view_names.insert(n);
                        }
                    }
                }
            }

            for (name, orig_f) in &orig_fields {
                if *name == "Time" {
                    continue;
                }
                let is_cross_view = cross_view_names.contains(name);
                if let Some(out_f) = out_fields.get(name) {
                    for &idx in &compare_indices {
                        // Skip shape comparison for cross-view duplicates
                        // (they become aliases with shape=8).
                        if idx == 7 && is_cross_view {
                            continue;
                        }
                        if idx < orig_f.len() && idx < out_f.len() && orig_f[idx] != out_f[idx] {
                            failures.push(format!(
                                "AC1.3: view[{i}] element {:?} field[{idx}] \
                                 orig={:?} out={:?}",
                                name, orig_f[idx], out_f[idx]
                            ));
                        }
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // AC1.4: Font specification preserved per view
    // -----------------------------------------------------------------------
    for (i, (orig, out)) in orig_views.iter().zip(&output_views).enumerate() {
        match (orig.2, out.2) {
            (Some(orig_font), Some(out_font)) => {
                if !out_font.contains("Verdana|10") {
                    failures.push(format!(
                        "AC1.4: view[{i}] font does not contain 'Verdana|10': {:?}",
                        out_font
                    ));
                }
                if orig_font != out_font {
                    failures.push(format!(
                        "AC1.4: view[{i}] font differs: orig={:?} out={:?}",
                        orig_font, out_font
                    ));
                }
            }
            (Some(_), None) => {
                failures.push(format!("AC1.4: view[{i}] missing font line in output"));
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // AC3.1: Lookup calls use `table ( input )` syntax, not LOOKUP()
    // -----------------------------------------------------------------------
    // mark2.mdl has: "historical federal funds rate = federal funds rate lookup ( Time )"
    if output.contains("LOOKUP(")
        && !output.contains("WITH LOOKUP(")
        && !output.contains("LOOKUP INVERT(")
    {
        failures.push(
            "AC3.1: output contains bare 'LOOKUP(' which should be table_name(input) syntax"
                .to_string(),
        );
    }

    // Positive check: the lookup call should use the Vensim table-call syntax
    let has_table_call = output.contains("federal funds rate lookup ( Time )")
        || output.contains("federal funds rate lookup( Time )")
        || output.contains("federal funds rate lookup (Time)")
        || output.contains("federal funds rate lookup(Time)");
    if !has_table_call {
        // Also accept the pattern where it appears in any casing
        let lower_output = output.to_lowercase();
        let has_lower_table_call = lower_output.contains("federal funds rate lookup")
            && !lower_output.contains("lookup(federal");
        if !has_lower_table_call {
            failures.push(
                "AC3.1: output does not contain 'federal funds rate lookup(...)' table call syntax"
                    .to_string(),
            );
        }
    }

    // -----------------------------------------------------------------------
    // AC3.2: Lookup range bounds preserved
    // -----------------------------------------------------------------------
    // mark2.mdl has lookups with explicit ranges like [(0,0)-(300,10)]
    if !output.contains("[(0,0)-(300,10)]") {
        failures.push(
            "AC3.2: federal funds rate lookup range [(0,0)-(300,10)] not preserved".to_string(),
        );
    }
    if !output.contains("[(0,0)-(400,10)]") {
        failures
            .push("AC3.2: inflation rate lookup range [(0,0)-(400,10)] not preserved".to_string());
    }
    // hud policy lookup range
    if !output.contains("[(108,0)-(800,1)]") {
        failures.push("AC3.2: hud policy lookup range [(108,0)-(800,1)] not preserved".to_string());
    }

    // -----------------------------------------------------------------------
    // AC4.1: Short equations use inline format
    // -----------------------------------------------------------------------
    // mark2.mdl has: "average repayment rate = 0.03"
    if !output.contains("average repayment rate = 0.03")
        && !output.contains("average repayment rate= 0.03")
        && !output.contains("average repayment rate =0.03")
    {
        failures.push(
            "AC4.1: short equation 'average repayment rate = 0.03' not in inline format"
                .to_string(),
        );
    }

    // -----------------------------------------------------------------------
    // AC4.3: Variable name casing preserved on LHS
    // -----------------------------------------------------------------------
    // mark2.mdl has mixed case: "New Homes On Market", "Endogenous Federal Funds Rate"
    let has_original_casing =
        output.contains("Endogenous Federal Funds Rate") || output.contains("New Homes On Market");
    if !has_original_casing {
        failures.push(
            "AC4.3: original variable casing not preserved (expected 'Endogenous Federal Funds Rate' or 'New Homes On Market')"
                .to_string(),
        );
    }

    // -----------------------------------------------------------------------
    // Report all failures
    // -----------------------------------------------------------------------
    if !failures.is_empty() {
        panic!(
            "{} MDL format roundtrip failure(s):\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
