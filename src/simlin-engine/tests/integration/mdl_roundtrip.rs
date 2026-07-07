// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::BufReader;

use simlin_engine::datamodel::{self, Dt, Project, Variable, ViewElement};
use simlin_engine::{mdl, xmile};

// MDL test files that should survive a full MDL -> Project -> MDL -> Project roundtrip
// at the equation/semantic level.
//
// Excluded categories:
//   - external data (GET DATA, GET XLS, etc.) -- external file references
//   - ALLOCATE / INVERT MATRIX -- unsupported builtins
//   - models with `inf` in unit ranges -- parser limitation
//
// Note: `xidz_zidz`, `lookups`, and `lookups_inline` were previously excluded
// (XIDZ 3-arg -> SAFEDIV argument-count drift, and inline lookup definitions the
// writer emitted in a form the parser could not re-parse). All three now survive
// the MDL -> Project -> MDL -> Project semantic roundtrip and are tested below.
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
    "test/test-models/tests/xidz_zidz/xidz_zidz.mdl",
    // graphical functions / lookups
    "test/test-models/tests/lookups/test_lookups.mdl",
    "test/test-models/tests/lookups_inline/test_lookups_inline.mdl",
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
    // macros (:MACRO: blocks + multi-output `:` reconstruction)
    "test/test-models/tests/macro_expression/test_macro_expression.mdl",
    "test/test-models/tests/macro_multi_expression/test_macro_multi_expression.mdl",
    "test/test-models/tests/macro_cross_reference/test_macro_cross_reference.mdl",
    "test/test-models/tests/macro_multi_macros/test_macro_multi_macros.mdl",
    "test/test-models/tests/macro_stock/test_macro_stock.mdl",
    "test/test-models/tests/macro_trailing_definition/test_macro_trailing_definition.mdl",
    "test/test-models/tests/macro_multi_output/test_macro_multi_output.mdl",
    "test/test-models/tests/macro_arrayed/test_macro_arrayed.mdl",
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
    // A dropped `:MACRO:` block or a malformed `MacroSpec` would otherwise
    // slip through: the body variables live in the macro model, so a
    // missing macro model only changes the model *count* (already checked),
    // but a present-but-wrong `MacroSpec` (bad parameters / outputs) would
    // not. Comparing `macro_spec` and `name` makes the macro round-trip
    // actually verified. For the non-macro fixtures these are no-ops (the
    // `main` model is named `"main"` with `macro_spec: None` on both
    // passes).
    if ma.name != mb.name {
        return Some(format!(
            "{path}: model[{i}] name differs: {:?} vs {:?}",
            ma.name, mb.name
        ));
    }
    if ma.macro_spec != mb.macro_spec {
        return Some(format!(
            "{path}: model[{i}] ({:?}) macro_spec differs: {:?} vs {:?}",
            ma.name, ma.macro_spec, mb.macro_spec
        ));
    }

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

#[test]
fn default_project_fishbanks_xmile_to_mdl_roundtrip() {
    let path = "default_projects/fishbanks/model.xmile";
    let file_path = resolve_path(path);
    let source = fs::read_to_string(&file_path).expect("read fishbanks model");

    let mut reader = BufReader::new(source.as_bytes());
    let xmile_project = xmile::project_from_reader(&mut reader).expect("parse fishbanks xmile");
    assert_eq!(
        xmile_project.models.len(),
        1,
        "fishbanks should be a single-model project"
    );

    let mdl_text = mdl::project_to_mdl(&xmile_project).expect("write fishbanks mdl");
    let mdl_project = mdl::parse_mdl(&mdl_text).expect("re-parse fishbanks mdl");
    assert_eq!(
        mdl_project.models.len(),
        1,
        "fishbanks mdl should stay single-model"
    );
    assert!(
        !mdl_project.models[0].views.is_empty(),
        "fishbanks mdl should contain a view"
    );

    let expected_names: HashSet<_> = xmile_project.models[0]
        .variables
        .iter()
        .map(|var| canonical_name(var.get_ident()))
        .collect();
    let actual_names: HashSet<_> = mdl_project.models[0]
        .variables
        .iter()
        .map(|var| canonical_name(var.get_ident()))
        .collect();
    let missing: Vec<_> = expected_names.difference(&actual_names).cloned().collect();
    assert!(
        missing.is_empty(),
        "{path}: missing variables after XMILE->MDL->MDL parse roundtrip: {missing:?}"
    );

    // The MDL sketch format stores a flow's name as a separate element rather
    // than a `label_side`; the parser recovers the side from where that element
    // sits relative to the valve. "New fish per year" is the one fishbanks flow
    // with label_side="top", so it exercises that recovery -- the rest are
    // "bottom". A regression here would silently flip flow labels to the other
    // side of the pipe on every Vensim round trip.
    let flow_label_side = |name: &str| -> datamodel::view_element::LabelSide {
        mdl_project.models[0]
            .views
            .iter()
            .flat_map(|view| match view {
                datamodel::View::StockFlow(sf) => sf.elements.iter(),
            })
            .find_map(|el| match el {
                ViewElement::Flow(f) if canonical_name(&f.name) == canonical_name(name) => {
                    Some(f.label_side)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{path}: flow {name:?} missing from roundtripped view"))
    };
    assert_eq!(
        flow_label_side("New fish per year"),
        datamodel::view_element::LabelSide::Top,
        "{path}: label_side=\"top\" flow should survive the XMILE->MDL->MDL roundtrip"
    );
    assert_eq!(
        flow_label_side("Harvest Rate"),
        datamodel::view_element::LabelSide::Bottom,
        "{path}: label_side=\"bottom\" flow should stay below its valve"
    );
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

        let source_views = split_sketch_into_views(&source);
        let output_views = split_sketch_into_views(&mdl_text);
        if source_views.len() != output_views.len() {
            failures.push(format!(
                "{path}: raw sketch view count differs: {} vs {}",
                source_views.len(),
                output_views.len()
            ));
        } else {
            for (j, (source_view, output_view)) in
                source_views.iter().zip(&output_views).enumerate()
            {
                if source_view.name != output_view.name {
                    failures.push(format!(
                        "{path}: raw sketch view[{j}] name differs: {:?} vs {:?}",
                        source_view.name, output_view.name
                    ));
                }

                if source_view.font_line != output_view.font_line {
                    failures.push(format!(
                        "{path}: raw sketch view[{j}] font differs: {:?} vs {:?}",
                        source_view.font_line, output_view.font_line
                    ));
                }

                let expected_named: Vec<_> = source_view
                    .element_lines
                    .iter()
                    .filter_map(|line| normalize_named_sketch_line(line))
                    .collect();
                let actual_named: Vec<_> = output_view
                    .element_lines
                    .iter()
                    .filter_map(|line| normalize_named_sketch_line(line))
                    .collect();
                if let Some(diff) = diff_multiset(&expected_named, &actual_named) {
                    failures.push(format!(
                        "{path}: raw sketch view[{j}] named lines differ: {diff}"
                    ));
                }
            }
        }

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

/// Verify that the mark2 model's sketch roundtrips with the ordering Vensim's
/// reader requires *within each view*:
///   - records appear in strictly increasing UID order (the UID is an array
///     offset; Vensim tolerates gaps but not backward references);
///   - a flow's pipe connectors (type 1 with flag 22) precede its valve
///     (type 11), which precedes its label (type 10, shape 40);
///   - a cloud (type 12) precedes the pipe connectors that reference it;
///   - pipe-connector direction flags are 4 (sink) or 100 (source) and
///     influence connectors carry field 9 == 64.
#[test]
fn mark2_sketch_ordering() {
    let file_path = resolve_path("test/bobby/vdf/econ/mark2.mdl");
    let source = fs::read_to_string(&file_path).expect("read mark2.mdl");
    let project = mdl::parse_mdl(&source).expect("parse mark2.mdl");

    let mdl_text = mdl::project_to_mdl(&project).expect("write mark2.mdl");

    // The written MDL should be re-parseable, and keep both views.
    let project2 = mdl::parse_mdl(&mdl_text).expect("re-parse mark2.mdl");
    assert_eq!(
        project.models[0].views.len(),
        project2.models[0].views.len()
    );

    let views = split_sketch_into_views(&mdl_text);
    assert!(
        !views.is_empty(),
        "written MDL should contain a sketch view"
    );

    // UIDs and cross-references share a per-view namespace, so every check is
    // scoped to one view's element lines.
    for view in &views {
        let lines = &view.element_lines;
        let fields_at = |idx: usize| -> Vec<&str> { lines[idx].split(',').collect() };
        let uid_of = |idx: usize| -> i32 { fields_at(idx)[1].parse().expect("record UID") };

        // (a) records appear in strictly increasing UID order.
        for window in (0..lines.len()).collect::<Vec<_>>().windows(2) {
            assert!(
                uid_of(window[0]) < uid_of(window[1]),
                "view {:?}: sketch records must be in increasing UID order, but \
                 {:?} follows {:?}",
                view.name,
                lines[window[1]],
                lines[window[0]],
            );
        }

        // (b) pipe-connector direction flags; (c) influence-connector field 9.
        for line in lines {
            if !line.starts_with("1,") {
                continue;
            }
            let fields: Vec<&str> = line.split(',').collect();
            if fields.len() <= 9 {
                continue;
            }
            if fields[7] == "22" {
                let direction: i32 = fields[4].parse().unwrap_or(0);
                assert!(
                    direction == 4 || direction == 100,
                    "view {:?}: pipe connector direction should be 4 or 100, got {direction}: {line}",
                    view.name,
                );
            } else {
                let influence_flag: i32 = fields[9].parse().unwrap_or(0);
                assert_eq!(
                    influence_flag, 64,
                    "view {:?}: influence connector should have field 9 == 64: {line}",
                    view.name,
                );
            }
        }

        // (d) pipe connectors precede their valve.
        for (i, line) in lines.iter().enumerate() {
            if !line.starts_with("11,") {
                continue;
            }
            let valve_uid = fields_at(i)[1];
            for (j, pipe_line) in lines.iter().enumerate() {
                let pipe_fields: Vec<&str> = pipe_line.split(',').collect();
                if pipe_line.starts_with("1,")
                    && pipe_fields.len() > 7
                    && pipe_fields[7] == "22"
                    && pipe_fields[2] == valve_uid
                {
                    assert!(
                        j < i,
                        "view {:?}: pipe connector for valve {valve_uid} at line {j} should \
                         precede the valve at line {i}",
                        view.name,
                    );
                }
            }
        }

        // (e) clouds precede the pipe connectors that reference them.
        for (i, line) in lines.iter().enumerate() {
            if !line.starts_with("12,") {
                continue;
            }
            let cloud_uid = fields_at(i)[1];
            for (j, pipe_line) in lines.iter().enumerate() {
                let pipe_fields: Vec<&str> = pipe_line.split(',').collect();
                if pipe_line.starts_with("1,")
                    && pipe_fields.len() > 7
                    && pipe_fields[7] == "22"
                    && pipe_fields[3] == cloud_uid
                {
                    assert!(
                        i < j,
                        "view {:?}: cloud {cloud_uid} at line {i} should precede the pipe \
                         connector that references it at line {j}",
                        view.name,
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Task 5: MDL format roundtrip (per-view element fidelity)
// ---------------------------------------------------------------------------

/// Split the sketch section of an MDL string into per-view segments.
#[derive(Debug, Clone)]
struct SketchView {
    name: String,
    element_lines: Vec<String>,
    font_line: Option<String>,
}

fn split_sketch_into_views(mdl_text: &str) -> Vec<SketchView> {
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
        let mut view_name = String::new();
        let mut font_line = None;
        let mut element_lines = Vec::new();

        for line in &lines {
            if let Some(name) = line.strip_prefix('*') {
                view_name = name.to_owned();
            } else if let Some(font) = line.strip_prefix('$') {
                font_line = Some(font.to_owned());
            } else if line.starts_with("10,")
                || line.starts_with("11,")
                || line.starts_with("12,")
                || line.starts_with("1,")
            {
                element_lines.push((*line).to_owned());
            }
        }

        views.push(SketchView {
            name: view_name,
            element_lines,
            font_line,
        });
    }

    views
}

fn line_fields(line: &str) -> Vec<&str> {
    line.split(',').collect()
}

/// Extract the variable name from a type-10 sketch element line.
/// Type-10 lines have format: 10,uid,name,x,y,...
fn extract_element_name(line: &str) -> Option<&str> {
    let fields = line_fields(line);
    if fields.len() > 2 && fields[0] == "10" {
        Some(fields[2])
    } else {
        None
    }
}

fn is_time_shadow_line(line: &str) -> bool {
    extract_element_name(line) == Some("Time")
}

fn is_flow_label_line(line: &str) -> bool {
    let fields = line_fields(line);
    fields.len() > 7 && fields[0] == "10" && fields[7] == "40"
}

fn is_pipe_connector(line: &str) -> bool {
    let fields = line_fields(line);
    fields.len() > 7 && fields[0] == "1" && fields[7] == "22"
}

fn is_influence_connector(line: &str) -> bool {
    let fields = line_fields(line);
    fields.len() > 7 && fields[0] == "1" && fields[7] != "22"
}

fn normalize_named_sketch_line(line: &str) -> Option<String> {
    if is_time_shadow_line(line) {
        return None;
    }
    if is_flow_label_line(line) {
        return None;
    }
    let fields = line_fields(line);
    if fields.len() > 3 && fields[0] == "10" {
        Some(format!("10,{},{}", fields[2], fields[3..].join(",")))
    } else {
        None
    }
}

fn normalize_flow_label_line(line: &str) -> Option<String> {
    let fields = line_fields(line);
    if fields.len() > 5 && fields[0] == "10" {
        Some(format!("10,{},{}", fields[2], fields[5..].join(",")))
    } else {
        None
    }
}

fn normalize_valve_line(line: &str) -> String {
    let fields = line_fields(line);
    format!("11,{}", fields[2..].join(","))
}

fn normalize_cloud_line(line: &str) -> String {
    let fields = line_fields(line);
    format!("12,{}", fields[2..].join(","))
}

fn uid_field(line: &str) -> Option<&str> {
    line_fields(line).get(1).copied()
}

fn multiset(items: &[String]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for item in items {
        *counts.entry(item.clone()).or_insert(0) += 1;
    }
    counts
}

fn multiset_delta(lhs: &HashMap<String, usize>, rhs: &HashMap<String, usize>) -> Vec<String> {
    let mut delta = Vec::new();
    let mut items: Vec<_> = lhs.iter().collect();
    items.sort_by_key(|(a, _)| *a);
    for (item, lhs_count) in items {
        let rhs_count = rhs.get(item).copied().unwrap_or(0);
        for _ in 0..lhs_count.saturating_sub(rhs_count) {
            delta.push(item.clone());
        }
    }
    delta
}

fn preview_items(items: &[String]) -> String {
    const LIMIT: usize = 4;
    if items.is_empty() {
        return "[]".to_owned();
    }
    let shown = items
        .iter()
        .take(LIMIT)
        .cloned()
        .collect::<Vec<_>>()
        .join(" | ");
    if items.len() > LIMIT {
        format!("[{shown} | ... +{} more]", items.len() - LIMIT)
    } else {
        format!("[{shown}]")
    }
}

fn diff_multiset(expected: &[String], actual: &[String]) -> Option<String> {
    let expected_counts = multiset(expected);
    let actual_counts = multiset(actual);
    let missing = multiset_delta(&expected_counts, &actual_counts);
    let extra = multiset_delta(&actual_counts, &expected_counts);
    if missing.is_empty() && extra.is_empty() {
        None
    } else {
        Some(format!(
            "missing={} extra={}",
            preview_items(&missing),
            preview_items(&extra)
        ))
    }
}

#[derive(Debug, Clone)]
struct FlowBlock {
    name: String,
    valve_uid: String,
    valve_line: String,
    label_line: String,
    cloud_lines: Vec<String>,
    pipe_lines: Vec<String>,
}

fn parse_flow_blocks(view: &SketchView) -> Result<HashMap<String, FlowBlock>, String> {
    let mut blocks = HashMap::new();
    for (idx, line) in view.element_lines.iter().enumerate() {
        if !line.starts_with("11,") {
            continue;
        }

        let Some(label_line) = view.element_lines.get(idx + 1) else {
            return Err(format!(
                "view {:?}: valve at line {} is missing its flow label",
                view.name, idx
            ));
        };
        if !is_flow_label_line(label_line) {
            return Err(format!(
                "view {:?}: valve at line {} is followed by a non-flow label line: {}",
                view.name, idx, label_line
            ));
        }

        let mut start = idx;
        while start > 0 {
            let prev = &view.element_lines[start - 1];
            if prev.starts_with("12,") || is_pipe_connector(prev) {
                start -= 1;
            } else {
                break;
            }
        }

        let name = extract_element_name(label_line)
            .ok_or_else(|| {
                format!(
                    "view {:?}: missing flow name for block at {}",
                    view.name, idx
                )
            })?
            .to_owned();
        let valve_uid = uid_field(line)
            .ok_or_else(|| {
                format!(
                    "view {:?}: missing valve uid for flow {:?}",
                    view.name, name
                )
            })?
            .to_owned();

        let mut cloud_lines = Vec::new();
        let mut pipe_lines = Vec::new();
        for block_line in &view.element_lines[start..idx] {
            if block_line.starts_with("12,") {
                cloud_lines.push(block_line.clone());
            } else if is_pipe_connector(block_line) {
                pipe_lines.push(block_line.clone());
            } else {
                return Err(format!(
                    "view {:?}: unexpected line inside flow block {:?}: {}",
                    view.name, name, block_line
                ));
            }
        }

        let block = FlowBlock {
            name: name.clone(),
            valve_uid,
            valve_line: line.clone(),
            label_line: label_line.clone(),
            cloud_lines,
            pipe_lines,
        };
        if blocks.insert(name.clone(), block).is_some() {
            return Err(format!(
                "view {:?}: duplicate flow block for {:?}",
                view.name, name
            ));
        }
    }
    Ok(blocks)
}

fn build_named_uid_map(lines: &[String]) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for line in lines {
        if let (Some(uid), Some(name)) = (uid_field(line), extract_element_name(line)) {
            names.insert(uid.to_owned(), name.to_owned());
        }
    }
    names
}

fn normalize_pipe_line(
    line: &str,
    valve_uid: &str,
    named_uids: &HashMap<String, String>,
    cloud_uids: &HashMap<String, String>,
) -> String {
    let fields = line_fields(line);
    let endpoint_uid = fields[3];
    if endpoint_uid == valve_uid {
        return "1,bend".to_owned();
    }
    let endpoint = named_uids
        .get(endpoint_uid)
        .cloned()
        .or_else(|| cloud_uids.get(endpoint_uid).cloned())
        .unwrap_or_else(|| format!("uid:{endpoint_uid}"));
    format!("1,to={endpoint}")
}

fn connector_shape(line: &str) -> String {
    let tail = line.split(",,").nth(1).unwrap_or_default();
    let point_count = tail.split('|').next().unwrap_or_default();
    if point_count == "1" {
        if tail.contains("(0,0)|") {
            "straight".to_owned()
        } else {
            "arc".to_owned()
        }
    } else if point_count.is_empty() {
        "unknown".to_owned()
    } else {
        format!("multipoint:{point_count}")
    }
}

fn normalize_flow_blocks(view: &SketchView) -> Result<HashMap<String, Vec<String>>, String> {
    let named_uids = build_named_uid_map(&view.element_lines);
    let flow_blocks = parse_flow_blocks(view)?;
    let mut normalized = HashMap::new();

    for (name, block) in flow_blocks {
        let mut cloud_uids = HashMap::new();
        for (idx, cloud_line) in block.cloud_lines.iter().enumerate() {
            let cloud_uid = uid_field(cloud_line).ok_or_else(|| {
                format!(
                    "view {:?}: cloud line in flow {:?} is missing a uid: {}",
                    view.name, block.name, cloud_line
                )
            })?;
            let label = if block.cloud_lines.len() == 1 {
                format!("cloud:{name}")
            } else {
                format!("cloud:{name}:{}", idx + 1)
            };
            cloud_uids.insert(cloud_uid.to_owned(), label);
        }

        let mut records = vec![
            format!(
                "label:{}",
                normalize_flow_label_line(&block.label_line).ok_or_else(|| {
                    format!(
                        "view {:?}: flow label for {:?} did not normalize: {}",
                        view.name, block.name, block.label_line
                    )
                })?
            ),
            format!("valve:{}", normalize_valve_line(&block.valve_line)),
        ];

        let mut clouds: Vec<_> = block
            .cloud_lines
            .iter()
            .map(|line| format!("cloud:{}", normalize_cloud_line(line)))
            .collect();
        clouds.sort();
        records.extend(clouds);

        let mut pipes: Vec<_> = block
            .pipe_lines
            .iter()
            .map(|line| {
                format!(
                    "pipe:{}",
                    normalize_pipe_line(line, &block.valve_uid, &named_uids, &cloud_uids)
                )
            })
            .collect();
        pipes.sort();
        records.extend(pipes);

        records.sort();
        normalized.insert(name, records);
    }

    Ok(normalized)
}

fn normalize_influence_connectors(view: &SketchView) -> Result<Vec<String>, String> {
    let mut uid_labels = build_named_uid_map(&view.element_lines);
    for (flow_name, block) in parse_flow_blocks(view)? {
        uid_labels.insert(block.valve_uid.clone(), flow_name.clone());
        if let Some(label_uid) = uid_field(&block.label_line) {
            uid_labels.insert(label_uid.to_owned(), flow_name.clone());
        }
        for (idx, cloud_line) in block.cloud_lines.iter().enumerate() {
            let cloud_uid = uid_field(cloud_line).ok_or_else(|| {
                format!(
                    "view {:?}: cloud line in flow {:?} is missing a uid: {}",
                    view.name, flow_name, cloud_line
                )
            })?;
            let label = if block.cloud_lines.len() == 1 {
                format!("cloud:{flow_name}")
            } else {
                format!("cloud:{flow_name}:{}", idx + 1)
            };
            uid_labels.insert(cloud_uid.to_owned(), label);
        }
    }

    let mut connectors = Vec::new();
    for line in view
        .element_lines
        .iter()
        .filter(|line| is_influence_connector(line))
    {
        let fields = line_fields(line);
        let from = uid_labels
            .get(fields[2])
            .cloned()
            .unwrap_or_else(|| format!("uid:{}", fields[2]));
        let to = uid_labels
            .get(fields[3])
            .cloned()
            .unwrap_or_else(|| format!("uid:{}", fields[3]));
        if from == "Time" || to == "Time" {
            continue;
        }
        connectors.push(format!(
            "1,{from}->{to},pol={},shape={}",
            fields.get(6).copied().unwrap_or("0"),
            connector_shape(line)
        ));
    }
    connectors.sort();
    Ok(connectors)
}

/// Verify mark2.mdl format roundtrip: parse, write, and compare the
/// output against the original at the per-view-element level.
///
/// Checks:
/// - AC1.1: Exactly 2 views with correct names
/// - AC1.2: Raw named sketch lines match as unordered multisets
/// - AC1.3: Flow blocks preserve semantic valve/cloud/pipe structure
/// - AC1.4: Influence connectors preserve resolved endpoint references and shape
/// - AC1.5: Font specification preserved per view
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
        let expected_names = ["1 housing", "2 investments"];
        for (i, expected) in expected_names.iter().enumerate() {
            if output_views[i].name != *expected {
                failures.push(format!(
                    "AC1.1: view[{i}] name {:?} != {:?}",
                    output_views[i].name, expected
                ));
            }
        }
    }

    // -----------------------------------------------------------------------
    // AC1.2: Raw named sketch lines match as unordered multisets
    // -----------------------------------------------------------------------
    if orig_views.len() == output_views.len() {
        for (i, (orig, out)) in orig_views.iter().zip(&output_views).enumerate() {
            let expected_named: Vec<_> = orig
                .element_lines
                .iter()
                .filter_map(|line| normalize_named_sketch_line(line))
                .collect();
            let actual_named: Vec<_> = out
                .element_lines
                .iter()
                .filter_map(|line| normalize_named_sketch_line(line))
                .collect();
            if let Some(diff) = diff_multiset(&expected_named, &actual_named) {
                failures.push(format!(
                    "AC1.2: view[{i}] ({:?}) raw named sketch lines differ: {diff}",
                    orig.name
                ));
            }
        }
    }

    // -----------------------------------------------------------------------
    // AC1.3: Flow blocks preserve raw valve/cloud/pipe records
    // -----------------------------------------------------------------------
    if orig_views.len() == output_views.len() {
        for (i, (orig, out)) in orig_views.iter().zip(&output_views).enumerate() {
            match (normalize_flow_blocks(orig), normalize_flow_blocks(out)) {
                (Ok(expected_blocks), Ok(actual_blocks)) => {
                    let expected_names: HashSet<_> = expected_blocks.keys().cloned().collect();
                    let actual_names: HashSet<_> = actual_blocks.keys().cloned().collect();

                    for missing_name in expected_names.difference(&actual_names) {
                        failures.push(format!(
                            "AC1.3: view[{i}] ({:?}) missing flow block {:?}",
                            orig.name, missing_name
                        ));
                    }
                    for extra_name in actual_names.difference(&expected_names) {
                        failures.push(format!(
                            "AC1.3: view[{i}] ({:?}) has extra flow block {:?}",
                            out.name, extra_name
                        ));
                    }

                    for flow_name in expected_names.intersection(&actual_names) {
                        let expected = expected_blocks
                            .get(flow_name)
                            .expect("expected flow block by name");
                        let actual = actual_blocks
                            .get(flow_name)
                            .expect("actual flow block by name");
                        if let Some(diff) = diff_multiset(expected, actual) {
                            failures.push(format!(
                                "AC1.3: view[{i}] ({:?}) flow block {:?} differs: {diff}",
                                orig.name, flow_name
                            ));
                        }
                    }
                }
                (Err(e), _) => failures.push(format!("AC1.3: source flow block parse failed: {e}")),
                (_, Err(e)) => failures.push(format!("AC1.3: output flow block parse failed: {e}")),
            }
        }
    }

    // -----------------------------------------------------------------------
    // AC1.4: Influence connectors preserve resolved endpoint references
    // -----------------------------------------------------------------------
    for (i, (orig, out)) in orig_views.iter().zip(&output_views).enumerate() {
        match (
            normalize_influence_connectors(orig),
            normalize_influence_connectors(out),
        ) {
            (Ok(expected), Ok(actual)) => {
                if let Some(diff) = diff_multiset(&expected, &actual) {
                    failures.push(format!(
                        "AC1.4: view[{i}] ({:?}) influence connectors differ: {diff}",
                        orig.name
                    ));
                }
            }
            (Err(e), _) => {
                failures.push(format!("AC1.4: source connector normalization failed: {e}"))
            }
            (_, Err(e)) => {
                failures.push(format!("AC1.4: output connector normalization failed: {e}"))
            }
        }
    }

    // -----------------------------------------------------------------------
    // AC1.5: Font specification preserved per view
    // -----------------------------------------------------------------------
    for (i, (orig, out)) in orig_views.iter().zip(&output_views).enumerate() {
        match (&orig.font_line, &out.font_line) {
            (Some(orig_font), Some(out_font)) => {
                if !out_font.contains("Verdana|10") {
                    failures.push(format!(
                        "AC1.5: view[{i}] font does not contain 'Verdana|10': {:?}",
                        out_font
                    ));
                }
                if orig_font != out_font {
                    failures.push(format!(
                        "AC1.5: view[{i}] font differs: orig={:?} out={:?}",
                        orig_font, out_font
                    ));
                }
            }
            (Some(_), None) => {
                failures.push(format!("AC1.5: view[{i}] missing font line in output"));
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

// ---------------------------------------------------------------------------
// Task 6: corpus-wide writer "ratchet" gates
// ---------------------------------------------------------------------------
//
// These two tests walk the whole `.mdl` corpus and pin the writer's current
// behavior so it can only get better, never regress. They are ratchets, not
// aspirational goals: each maintains an allowlist of the fixtures that the
// writer currently mishandles, and asserts BOTH that no *un*listed fixture
// misbehaves (catches regressions) AND that every listed fixture *still*
// misbehaves (catches a fix that forgot to shrink the allowlist). A later
// writer fix therefore MUST delete the entries it repairs, or the gate fails
// loudly -- which is the whole point: the allowlists shrink to empty as the
// writer is hardened (GH #851).
//
// Performance: the walk parses+writes ~230 fixtures in-process (never spawns
// the CLI). Fixtures larger than `MAX_FIXTURE_BYTES` are skipped so a debug
// build stays within the per-test "few seconds" budget; in practice this skips
// only `C-LEARN` (1.4 MB). Every other corpus file (the largest re-parseable
// one is ~195 KB) parses in tens of milliseconds, and the whole walk completes
// in ~2 s on a debug build.

/// Skip fixtures larger than this so the debug-build walk stays within the
/// per-test time budget. Only `test/xmutil_test_models/C-LEARN` (1.4 MB)
/// exceeds it today; every other corpus `.mdl` is comfortably under.
const MAX_FIXTURE_BYTES: u64 = 200 * 1024;

/// Minimum number of fixtures both ratchets must actually exercise. The ratchets
/// only make *allowlisted* breakage loud; a reader- or source-side regression
/// that made currently-clean, un-listed fixtures unparseable would silently
/// shrink coverage with no signal. This floor turns that collapse into a loud
/// failure. Set well below today's ~209 attempted/re-parseable so ordinary
/// corpus edits don't trip it, but high enough to catch a real collapse.
/// (Mirrors the `ltm_corpus_floor_gate` precedent.)
const MIN_CORPUS_FIXTURES: usize = 200;

/// Fixtures whose writer OUTPUT currently fails to re-parse. Calibrated
/// empirically by running `corpus_reparse_ratchet` and recording the actual
/// failures -- do NOT edit by guesswork.
///
/// This list is now EMPTY: the context-aware writer (`WriterContext` in
/// `mdl::writer`) closed both former root causes.
///
///   - wildcard/subrange subscripts: `SUM(a[*])` (and `a[*:Dim]`) now recover
///     Vensim's bang form `a[Dim!]` from the variable's declared dimensions
///     instead of emitting the reader-rejected `*` / `*:Dim` (#847).
///   - `PI()` builtin collision: a genuine `pi` builtin lowers to a numeric
///     literal, and a user variable named `PI`/`pi` is emitted as that
///     identifier -- neither produces the reader-rejected `PI()` (#850, #853).
///
/// Several fixtures cleared here still fail the IDEMPOTENCE ratchet for
/// unrelated, pre-existing reasons (CRLF free-text accumulation #849, sketch
/// connector instability); those are listed in `EXPECTED_NON_IDEMPOTENT`.
const EXPECTED_REPARSE_FAILURES: &[&str] = &[];

/// Re-parseable fixtures whose writer output is NOT a fixpoint: writing the
/// re-parsed model a second time changes the text. Calibrated empirically by
/// running `writer_output_idempotence_ratchet`. Grouped by the dominant
/// observed diff class (a file may exhibit more than one; it is listed under
/// its primary cause):
///
///   - sketch instability: the sketch (diagram) section is not a fixpoint --
///     connector UIDs/ordering, coordinates, or element name-quoting shift on
///     the second write. The equations are stable.
///   - CRLF free-text accumulation: a multi-line comment/units/doc field
///     gains an extra trailing carriage return each round -- the writer does
///     not strip embedded `\r` from free text before re-emitting it (GH #849).
///   - arrayed element order: the order of an arrayed variable's per-element
///     sub-equations is not stable across a round trip.
///   - builtin-call spacing: `SIZE(x)` is re-emitted as `SIZE ( x )`.
///   - unary-minus parenthesization: `-x ^ y` is re-emitted as `-(x ^ y)`.
///   - escape doubling: backslashes/quotes inside a quoted identifier are
///     re-escaped on each pass, so the escaping grows.
///   - control-variable value substitution: a reference to a control variable
///     (`TIME STEP`) is replaced by its literal value on the second write.
const EXPECTED_NON_IDEMPOTENT: &[&str] = &[
    // sketch instability
    "test/bobby/vdf/econ/mark2.mdl",
    "test/sdeverywhere/models/smooth3/smooth3.mdl",
    "test/test-models/samples/Population/Subscripted Population Model.mdl",
    "test/test-models/samples/Roessler_Chaos/roessler_chaos.mdl",
    "test/test-models/samples/Workforce/workforce.mdl",
    "test/test-models/tests/arguments/test_arguments.mdl",
    "test/test-models/tests/data_from_other_model/model_to_generate_data.mdl",
    "test/test-models/tests/data_from_other_model/test_data_from_other_model.mdl",
    "test/test-models/tests/delay_fixed/test_delay_fixed.mdl",
    "test/test-models/tests/delays/test_delays.mdl",
    "test/test-models/tests/except/test_except.mdl",
    "test/test-models/tests/function_capitalization/test_function_capitalization.mdl",
    "test/test-models/tests/logicals/test_logicals.mdl",
    "test/test-models/tests/lookups_inline_bounded/test_lookups_inline_bounded.mdl",
    "test/test-models/tests/macro_cross_reference/test_macro_cross_reference.mdl",
    "test/test-models/tests/macro_expression/test_macro_expression.mdl",
    "test/test-models/tests/macro_multi_expression/test_macro_multi_expression.mdl",
    "test/test-models/tests/macro_multi_macros/test_macro_multi_macros.mdl",
    "test/test-models/tests/macro_stock/test_macro_stock.mdl",
    "test/test-models/tests/macro_trailing_definition/test_macro_trailing_definition.mdl",
    "test/test-models/tests/rounding/test_rounding.mdl",
    "test/test-models/tests/sample_if_true/test_sample_if_true.mdl",
    "test/test-models/tests/smooth/test_smooth.mdl",
    "test/test-models/tests/subrange_merge/test_subrange_merge.mdl",
    "test/test-models/tests/subscript_multiples/test_multiple_subscripts.mdl",
    "test/test-models/tests/subscript_selection/subscript_selection.mdl",
    "test/test-models/tests/subscripted_if_then_else/test_subscripted_if_then_else.mdl",
    "test/test-models/tests/subscripted_logicals/test_subscripted_logicals.mdl",
    "test/test-models/tests/subscripted_lookups/test_subscripted_lookups.mdl",
    "test/test-models/tests/subscripted_round/test_subscripted_round.mdl",
    "test/test-models/tests/subscripted_xidz/test_subscripted_xidz.mdl",
    "test/test-models/tests/xidz_zidz/xidz_zidz.mdl",
    "test/test-models/tests/zeroled_decimals/test_zeroled_decimals.mdl",
    // CRLF free-text accumulation (GH #849)
    "test/metasd/WRLD3-03/wrld3-03.mdl",
    "test/metasd/bathtub-statistics/integration3.mdl",
    "test/metasd/covid19-us-homer/homer v8/Covid19US v8.mdl",
    "test/metasd/critical-slowing/critical-slowing.mdl",
    "test/metasd/early-warnings-catastrophe/catastropeWarning2.mdl",
    "test/metasd/pink-noise/PinkNoise2010.mdl",
    "test/metasd/theil-statistics/Theil_2011.mdl",
    "test/metasd/wonderland/Wonderland3.mdl",
    "test/sdeverywhere/models/active_initial/active_initial.mdl",
    "test/sdeverywhere/models/flatten/input1.mdl",
    "test/sdeverywhere/models/flatten/input2.mdl",
    "test/sdeverywhere/models/sir/model/sir.mdl",
    "test/sdeverywhere/models/sir/sir.mdl",
    "test/test-models/samples/Query_file/Query_file.mdl",
    "test/test-models/samples/SIR/SIR.mdl",
    "test/test-models/samples/teacup/teacup.mdl",
    // Uppercase `.MDL` twin of teacup; same CRLF free-text non-idempotence.
    "test/test-models/tests/case_sensitive_extension/teacup-upper.MDL",
    "test/test-models/tests/active_initial/test_active_initial.mdl",
    "test/test-models/tests/delay_pipeline/test_pipeline_delays.mdl",
    "test/test-models/tests/euler_step_vs_saveper/test_euler_step_vs_saveper.mdl",
    "test/test-models/tests/forecast/test_forecast.mdl",
    "test/test-models/tests/limits/test_limits.mdl",
    "test/test-models/tests/line_breaks/test_line_breaks.mdl",
    "test/test-models/tests/macro_arrayed/test_macro_arrayed.mdl",
    "test/test-models/tests/macro_clearn_ramp_from_to/test_macro_clearn_ramp_from_to.mdl",
    "test/test-models/tests/macro_clearn_sample_until/test_macro_clearn_sample_until.mdl",
    "test/test-models/tests/macro_clearn_sshape/test_macro_clearn_sshape.mdl",
    "test/test-models/tests/macro_multi_output/test_macro_multi_output.mdl",
    "test/test-models/tests/smooth_and_stock/test_smooth_and_stock.mdl",
    "test/test-models/tests/variable_ranges/test_variable_ranges.mdl",
    // arrayed element order
    "test/metasd/social-network-valuation/groupon 1.mdl",
    "test/metasd/social-network-valuation/groupon 2.mdl",
    "test/metasd/social-network-valuation/groupon 3.mdl",
    "test/test-models/tests/allocate_available/test_allocate_available.mdl",
    "test/test-models/tests/allocate_by_priority/test_allocate_by_priority.mdl",
    "test/test-models/tests/array_with_line_break/test_array_with_line_break.mdl",
    "test/test-models/tests/invert_matrix/test_invert_matrix.mdl",
    "test/test-models/tests/subscript_element_name/test_subscript_element_name.mdl",
    "test/test-models/tests/subscript_mapping_simple/test_subscript_mapping_simple.mdl",
    "test/test-models/tests/subscript_mapping_vensim/test_subscript_mapping_vensim.mdl",
    "test/test-models/tests/subscripted_delays/test_subscripted_delays.mdl",
    "test/test-models/tests/subscripted_smooth/test_subscripted_smooth.mdl",
    "test/test-models/tests/tabbed_arrays/tabbed_arrays.mdl",
    // builtin-call spacing (`SIZE(x)` -> `SIZE ( x )`)
    "test/sdeverywhere/models/elmcount/elmcount.mdl",
    "test/test-models/tests/elm_count/test_elm_count.mdl",
    "test/test-models/tests/subscript_definition/test_subscript_definition.mdl",
    // unary-minus parenthesization (`-x ^ y` -> `-(x ^ y)`)
    "test/test-models/tests/arithmetics/test_arithmetics.mdl",
    "test/test-models/tests/arithmetics_exp/test_arithmetics_exp.mdl",
    // escape doubling inside quoted identifiers
    "test/test-models/tests/special_characters/test_special_variable_names.mdl",
    // control-variable value substitution (`TIME STEP` -> its literal value)
    "test/test-models/tests/control_vars/test_control_vars.mdl",
    // Newly re-parseable after the context-aware writer landed (#847/#850/#853):
    // these fixtures used to fail the RE-PARSE ratchet (wildcard/subrange
    // subscripts or a PI collision), so their idempotence was never measured.
    // Now that they re-parse, a pre-existing non-idempotence surfaces -- almost
    // entirely CRLF free-text accumulation (#849) and/or sketch connector
    // control-point instability; the equations themselves round-trip. They are
    // recorded here (not a regression: they were never in the idempotent set)
    // and shrink as #849 and the sketch fixes land.
    "test/metasd/FREE/FREE6/FREE6-corrected/conversion.mdl",
    "test/metasd/FREE/FREE6/FREE6-corrected/conversion2.mdl",
    "test/metasd/FREE/FREE6/FREE6-original/conversion.mdl",
    "test/metasd/FREE/FREE6/FREE6-original/conversion2.mdl",
    "test/metasd/FREE/FREE6/FREE6-original/free 6.mdl",
    "test/metasd/beer-game/RealBeer4-Sterman13.mdl",
    "test/metasd/industrial-dynamics/IDch15/IDch15d.mdl",
    "test/metasd/interpolating-arrays/InterpolatingArrays.mdl",
    "test/metasd/scientific-revolution/scirev7.mdl",
    "test/metasd/scientific-revolution/scirev8.mdl",
    "test/metasd/thyroid-dynamics/thyroid-2008-d.mdl",
    "test/sdeverywhere/models/allocate/allocate.mdl",
    "test/sdeverywhere/models/arrays_cname/arrays_cname.mdl",
    "test/sdeverywhere/models/arrays_varname/arrays_varname.mdl",
    "test/sdeverywhere/models/longeqns/longeqns.mdl",
    "test/sdeverywhere/models/prune/prune.mdl",
    "test/sdeverywhere/models/sumif/sumif.mdl",
    "test/sdeverywhere/models/vector/vector.mdl",
    "test/test-models/tests/subscript_aggregation/test_subscript_aggregation.mdl",
    "test/test-models/tests/subscript_transposition/test_subscript_transposition.mdl",
];

/// Recursively collect every `.mdl` fixture under `test/`, returned as sorted,
/// repo-relative paths (e.g. `test/test-models/tests/abs/test_abs.mdl`). Uses
/// `std::fs` recursion only -- no extra dependency.
fn collect_mdl_fixtures() -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(resolve_path("test"))];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("mdl"))
            {
                let s = path.to_string_lossy();
                // Convert the on-disk path (prefixed with the crate-relative
                // `../../`) back to a repo-relative `test/...` path so it
                // matches the allowlists above.
                if let Some(idx) = s.find("test/") {
                    out.push(s[idx..].to_string());
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// CORPUS RE-PARSE RATCHET. For every corpus `.mdl` whose SOURCE parses (a
/// source that does not parse is skipped -- an external-data / GET DIRECT /
/// missing-CSV issue is not the writer's fault), assert that the writer's
/// output re-parses cleanly UNLESS the fixture is in `EXPECTED_REPARSE_FAILURES`.
/// The assertion runs both directions so the allowlist can never go stale:
///   (a) an un-listed fixture that fails to re-parse is a REGRESSION;
///   (b) a listed fixture that now re-parses cleanly means a writer fix landed
///       without shrinking the allowlist -- delete the entry.
#[test]
fn corpus_reparse_ratchet() {
    // Fixtures whose source parsed (so a writer round trip was actually
    // attempted) and, of those, the ones whose output failed to re-parse.
    let mut attempted: HashSet<String> = HashSet::new();
    let mut failed: HashSet<String> = HashSet::new();

    for path in collect_mdl_fixtures() {
        let file_path = resolve_path(&path);
        let Ok(meta) = fs::metadata(&file_path) else {
            continue;
        };
        if meta.len() > MAX_FIXTURE_BYTES {
            continue;
        }
        let Ok(source) = fs::read_to_string(&file_path) else {
            continue;
        };
        // A source that does not parse is a source-side / external-data issue,
        // not a writer bug: skip it entirely.
        let Ok(project) = mdl::parse_mdl(&source) else {
            continue;
        };
        attempted.insert(path.clone());

        // The output "re-parses cleanly" iff the writer produced text AND that
        // text parses. A writer error (never observed today) counts as a
        // failure because no re-parseable output exists.
        let reparses = mdl::project_to_mdl(&project)
            .ok()
            .is_some_and(|output| mdl::parse_mdl(&output).is_ok());
        if !reparses {
            failed.insert(path);
        }
    }

    // Coverage floor: a reader/source regression that silently unparsed most of
    // the corpus would otherwise make this ratchet pass vacuously (it only
    // asserts against the allowlist). Fail loudly if coverage collapses.
    assert!(
        attempted.len() >= MIN_CORPUS_FIXTURES,
        "corpus re-parse ratchet only attempted {} fixtures (floor {}); the corpus walk or the \
         shared MDL parser likely regressed",
        attempted.len(),
        MIN_CORPUS_FIXTURES,
    );

    let expected: HashSet<&str> = EXPECTED_REPARSE_FAILURES.iter().copied().collect();
    let mut problems: Vec<String> = Vec::new();

    // (a) Regressions: an attempted fixture not on the allowlist must re-parse.
    for path in &failed {
        if !expected.contains(path.as_str()) {
            problems.push(format!(
                "REGRESSION: {path} writer output no longer re-parses -- fix the writer \
                 (do not add it to the allowlist)"
            ));
        }
    }

    // (b) Stale allowlist entries: every listed path must have been attempted
    // and must still fail.
    for &path in EXPECTED_REPARSE_FAILURES {
        if !attempted.contains(path) {
            problems.push(format!(
                "STALE: allowlisted {path} was not attempted (source no longer parses, file \
                 moved/renamed, or exceeds the size cap) -- remove it from EXPECTED_REPARSE_FAILURES"
            ));
        } else if !failed.contains(path) {
            problems.push(format!(
                "FIXED: allowlisted {path} now re-parses cleanly -- remove it from \
                 EXPECTED_REPARSE_FAILURES"
            ));
        }
    }

    problems.sort();
    assert!(
        problems.is_empty(),
        "corpus re-parse ratchet ({} attempted, {} currently failing):\n{}",
        attempted.len(),
        failed.len(),
        problems.join("\n")
    );
}

/// IDEMPOTENCE RATCHET. Over the re-parseable set (source parses, writer output
/// parses), assert `write(parse(write(parse(x)))) == write(parse(x))` -- i.e.
/// the writer's output is a fixpoint -- UNLESS the fixture is in
/// `EXPECTED_NON_IDEMPOTENT`. Two-way, same contract as the re-parse ratchet:
///   (a) an un-listed fixture whose second write differs is a REGRESSION;
///   (b) a listed fixture that is now idempotent means a writer fix landed
///       without shrinking the allowlist -- delete the entry.
#[test]
fn writer_output_idempotence_ratchet() {
    // Re-parseable fixtures and, of those, the ones whose writer output is not
    // a fixpoint.
    let mut reparseable: HashSet<String> = HashSet::new();
    let mut non_idempotent: HashSet<String> = HashSet::new();

    for path in collect_mdl_fixtures() {
        let file_path = resolve_path(&path);
        let Ok(meta) = fs::metadata(&file_path) else {
            continue;
        };
        if meta.len() > MAX_FIXTURE_BYTES {
            continue;
        }
        let Ok(source) = fs::read_to_string(&file_path) else {
            continue;
        };
        let Ok(project1) = mdl::parse_mdl(&source) else {
            continue;
        };
        let Ok(write1) = mdl::project_to_mdl(&project1) else {
            continue;
        };
        // Idempotence is only defined over the re-parseable set: a fixture
        // whose first output does not parse is the re-parse ratchet's job.
        let Ok(project2) = mdl::parse_mdl(&write1) else {
            continue;
        };
        reparseable.insert(path.clone());

        let Ok(write2) = mdl::project_to_mdl(&project2) else {
            // Writing an already-parsed model should never error; treat it as
            // a non-idempotence so it surfaces rather than being swallowed.
            non_idempotent.insert(path);
            continue;
        };
        if write1 != write2 {
            non_idempotent.insert(path);
        }
    }

    // Coverage floor (see `corpus_reparse_ratchet`): guard against a silent
    // collapse of the re-parseable set.
    assert!(
        reparseable.len() >= MIN_CORPUS_FIXTURES,
        "idempotence ratchet only exercised {} re-parseable fixtures (floor {}); the corpus walk \
         or the shared MDL pipeline likely regressed",
        reparseable.len(),
        MIN_CORPUS_FIXTURES,
    );

    let expected: HashSet<&str> = EXPECTED_NON_IDEMPOTENT.iter().copied().collect();
    let mut problems: Vec<String> = Vec::new();

    // (a) Regressions.
    for path in &non_idempotent {
        if !expected.contains(path.as_str()) {
            problems.push(format!(
                "REGRESSION: {path} writer output is no longer a fixpoint -- fix the writer \
                 (do not add it to the allowlist)"
            ));
        }
    }

    // (b) Stale allowlist entries.
    for &path in EXPECTED_NON_IDEMPOTENT {
        if !reparseable.contains(path) {
            problems.push(format!(
                "STALE: allowlisted {path} is no longer in the re-parseable set (source or \
                 writer output no longer parses, file moved, or exceeds the size cap) -- remove \
                 it from EXPECTED_NON_IDEMPOTENT"
            ));
        } else if !non_idempotent.contains(path) {
            problems.push(format!(
                "FIXED: allowlisted {path} is now idempotent -- remove it from \
                 EXPECTED_NON_IDEMPOTENT"
            ));
        }
    }

    problems.sort();
    assert!(
        problems.is_empty(),
        "writer idempotence ratchet ({} re-parseable, {} non-idempotent):\n{}",
        reparseable.len(),
        non_idempotent.len(),
        problems.join("\n")
    );
}
