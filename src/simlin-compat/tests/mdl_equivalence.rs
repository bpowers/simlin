// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! MDL equivalence tests comparing xmutil-based parsing vs native Rust parsing.
//!
//! These tests verify that `open_vensim()` (MDL -> xmutil -> XMILE -> datamodel) produces
//! equivalent results to `open_vensim_native()` (MDL -> datamodel directly).
//!
//! ## Known Feature Gaps in Native Parser
//!
//! The following features are not yet implemented in the native MDL parser:
//! - TODO: View/diagram parsing (Model.views)
//! - TODO: Loop metadata extraction (Model.loop_metadata)
//! - TODO: Model-level sim_specs (currently only project-level is extracted)

#![cfg(feature = "xmutil")]

use std::collections::HashMap;
use std::fs;
use std::io::BufReader;

use simlin_compat::{open_vensim, open_vensim_native};
use simlin_core::canonicalize;
use simlin_core::datamodel::{
    Aux, Dimension, DimensionElements, Dt, Equation, Flow, Model, Module, Project, SimSpecs, Stock,
    Variable,
};

/// Models that should produce equivalent output from both parsers.
/// Add models here as they pass equivalence testing.
static EQUIVALENT_MODELS: &[&str] = &[
    // SIR model - known to work
    "src/libsimlin/testdata/SIR.mdl",
    // SDEverywhere models
    "test/sdeverywhere/models/active_initial/active_initial.mdl",
    "test/sdeverywhere/models/sir/sir.mdl",
    "test/sdeverywhere/models/comments/comments.mdl",
    "test/sdeverywhere/models/delay/delay.mdl",
    "test/sdeverywhere/models/elmcount/elmcount.mdl",
    "test/sdeverywhere/models/index/index.mdl",
    "test/sdeverywhere/models/initial/initial.mdl",
    "test/sdeverywhere/models/pulsetrain/pulsetrain.mdl",
    "test/sdeverywhere/models/smooth/smooth.mdl",
    // "test/sdeverywhere/models/smooth3/smooth3.mdl", // causes segfault in xmutil
    "test/sdeverywhere/models/specialchars/specialchars.mdl",
    // "test/sdeverywhere/models/subalias/subalias.mdl", // dimension mapping handling differs
    // "test/sdeverywhere/models/trend/trend.mdl", // causes segfault in xmutil
    // test-models samples
    "test/test-models/samples/SIR/SIR.mdl",
    "test/test-models/samples/teacup/teacup.mdl",
];

// ===== MODELS WITH KNOWN ISSUES =====
// Track failures with comments explaining the issue.
//
// Subscript handling in expressions differs (xmutil preserves "c[dima]",
// native uses "c"):
// "test/sdeverywhere/models/lookup/lookup.mdl"
//
// Uses ALLOCATE AVAILABLE builtin:
// "test/sdeverywhere/models/allocate/allocate.mdl"
//
// Uses GET DIRECT CONSTANTS:
// "test/sdeverywhere/models/arrays_cname/arrays_cname.mdl"
// "test/sdeverywhere/models/arrays_varname/arrays_varname.mdl"
//
// Uses DELAY FIXED builtin:
// "test/sdeverywhere/models/delayfixed/delayfixed.mdl"
// "test/sdeverywhere/models/delayfixed2/delayfixed2.mdl"
//
// Uses GET XLS DATA / GET DIRECT DATA:
// "test/sdeverywhere/models/directdata/directdata.mdl"
// "test/sdeverywhere/models/extdata/extdata.mdl"
//
// Uses GET DIRECT LOOKUPS:
// "test/sdeverywhere/models/directlookups/directlookups.mdl"
//
// Uses GET DIRECT SUBSCRIPT:
// "test/sdeverywhere/models/directsubs/directsubs.mdl"
//
// Uses EXCEPT syntax with subscript mappings:
// "test/sdeverywhere/models/except/except.mdl"
// "test/sdeverywhere/models/except2/except2.mdl"
//
// Uses GET DATA BETWEEN TIMES:
// "test/sdeverywhere/models/getdata/getdata.mdl"
//
// Uses subscript mappings:
// "test/sdeverywhere/models/mapping/mapping.mdl"
// "test/sdeverywhere/models/multimap/multimap.mdl"
// "test/sdeverywhere/models/subscript/subscript.mdl"
//
// Uses NPV builtin:
// "test/sdeverywhere/models/npv/npv.mdl"
//
// Uses QUANTUM builtin:
// "test/sdeverywhere/models/quantum/quantum.mdl"
//
// Uses SAMPLE IF TRUE builtin:
// "test/sdeverywhere/models/sample/sample.mdl"
//
// Preprocessing test files (no expected results):
// "test/sdeverywhere/models/flatten/expected.mdl"
// "test/sdeverywhere/models/flatten/input1.mdl"
// "test/sdeverywhere/models/preprocess/expected.mdl"
// "test/sdeverywhere/models/preprocess/input.mdl"

/// Check if a model name is a stdlib model (used for builtins like SMOOTH3, TREND).
fn is_stdlib_model(name: &str) -> bool {
    // Stdlib models have names starting with special Unicode characters
    name.starts_with('∫') || name.starts_with('⌈')
}

/// Normalize a dimension by lowercasing names (Vensim is case-insensitive).
fn normalize_dimension(dim: &mut Dimension) {
    dim.name = dim.name.to_lowercase();
    if let DimensionElements::Named(elements) = &mut dim.elements {
        for elem in elements.iter_mut() {
            *elem = elem.to_lowercase();
        }
    }
    if let Some(maps_to) = &mut dim.maps_to {
        *maps_to = maps_to.to_lowercase();
    }
}

/// Normalize sim_specs by handling differences in save_step representation.
/// When save_step equals dt, some parsers set it to None while others set it explicitly.
fn normalize_sim_specs(specs: &mut SimSpecs) {
    // If save_step equals dt, normalize to None (xmutil behavior)
    if let Some(save_step) = &specs.save_step {
        let dt_val = match &specs.dt {
            Dt::Dt(v) => *v,
            Dt::Reciprocal(v) => 1.0 / *v,
        };
        let save_val = match save_step {
            Dt::Dt(v) => *v,
            Dt::Reciprocal(v) => 1.0 / *v,
        };
        if (dt_val - save_val).abs() < 1e-10 {
            specs.save_step = None;
        }
    }
}

/// Normalize a project for comparison by clearing/sorting fields that may legitimately differ.
fn normalize_project(mut project: Project) -> Project {
    // Project name differs: xmutil extracts from filename, native may leave empty
    project.name = String::new();

    // Source is implementation-dependent
    project.source = None;

    // AI information is not relevant to MDL parsing
    project.ai_information = None;

    // Normalize sim_specs
    normalize_sim_specs(&mut project.sim_specs);

    // Unit equivalences match without normalization

    // Normalize dimensions (lowercase names for case-insensitive comparison)
    for dim in &mut project.dimensions {
        normalize_dimension(dim);
    }

    // Sort dimensions by name for consistent comparison
    project.dimensions.sort_by(|a, b| a.name.cmp(&b.name));

    // Filter out stdlib models and normalize the rest
    project.models.retain(|m| !is_stdlib_model(&m.name));
    for model in &mut project.models {
        normalize_model(model);
    }
    // Model ordering matches without sorting

    project
}

/// Normalize a model for comparison.
fn normalize_model(model: &mut Model) {
    // TODO: Views are diagram-related; native parser doesn't implement views yet.
    // When implemented, compare views instead of clearing.
    model.views.clear();

    // TODO: Loop metadata is diagram-related; native parser doesn't extract it yet.
    // When implemented, compare loop_metadata instead of clearing.
    model.loop_metadata.clear();

    // Normalize each variable
    for var in &mut model.variables {
        normalize_variable(var);
    }
    // Variable ordering matches without sorting
}

/// Clear view-related and AI-related fields from a variable.
fn normalize_variable(var: &mut Variable) {
    match var {
        Variable::Stock(stock) => normalize_stock(stock),
        Variable::Flow(flow) => normalize_flow(flow),
        Variable::Aux(aux) => normalize_aux(aux),
        Variable::Module(module) => normalize_module(module),
    }
}

/// Canonicalize an identifier to lowercase with underscores.
fn canonical_ident(ident: &str) -> String {
    canonicalize(ident).to_string()
}

/// Normalize documentation by removing line continuations and collapsing whitespace.
fn normalize_doc(doc: &str) -> String {
    // Remove Vensim line continuations: backslash followed by CR/LF and tabs
    let doc = doc.replace("\\\r\n", " ");
    let doc = doc.replace("\\\n", " ");
    // Collapse multiple whitespace (tabs, spaces) into single space
    let doc: String = doc.split_whitespace().collect::<Vec<_>>().join(" ");
    doc
}

/// Normalize units by removing spaces and underscores.
///
/// The production code in `format_unit_expr` now handles simplification and
/// canonical formatting, so this only needs to normalize whitespace differences.
fn normalize_units(units: Option<&String>) -> Option<String> {
    units.map(|u| u.replace([' ', '_'], ""))
}

/// Normalize an equation expression by:
/// - Lowercasing (function names and variable references)
/// - Removing spaces around operators
fn normalize_expr(expr: &str) -> String {
    // Lowercase the entire expression for case-insensitive comparison
    // (Vensim is case-insensitive, xmutil lowercases, native may preserve)
    let expr = expr.to_lowercase();
    // Remove spaces around operators: *, /, +, -, ^
    let expr = expr.replace(" * ", "*");
    let expr = expr.replace(" / ", "/");
    let expr = expr.replace(" + ", "+");
    let expr = expr.replace(" - ", "-");
    let expr = expr.replace(" ^ ", "^");
    expr.trim().to_string()
}

/// Normalize an equation.
/// For arrayed equations, sorts elements by subscript key to handle ordering differences
/// between native (sorted by subscript) and XMILE (preserves source order).
/// Also converts Arrayed equations where all elements have the same expression to ApplyToAll.
fn normalize_equation(eq: &mut Equation) {
    match eq {
        Equation::Scalar(expr, comment) => {
            *expr = normalize_expr(expr);
            if let Some(c) = comment {
                *c = normalize_doc(c);
            }
        }
        Equation::ApplyToAll(dims, expr, comment) => {
            // Lowercase and sort dimension names for consistent comparison
            for dim in dims.iter_mut() {
                *dim = dim.to_lowercase();
            }
            dims.sort();
            *expr = normalize_expr(expr);
            if let Some(c) = comment {
                *c = normalize_doc(c);
            }
        }
        Equation::Arrayed(dims, elements) => {
            // Lowercase and sort dimension names for consistent comparison
            for dim in dims.iter_mut() {
                *dim = dim.to_lowercase();
            }
            dims.sort();
            // Normalize each element
            for (subscript, expr, comment, gf) in elements.iter_mut() {
                // Canonicalize subscript names
                *subscript = canonical_ident(subscript);
                *expr = normalize_expr(expr);
                if let Some(c) = comment {
                    *c = normalize_doc(c);
                }
                // Graphical functions match without normalization
                let _ = gf;
            }
            // Element ordering and comments match without normalization

            // Check if all elements have the same expression - if so, convert to ApplyToAll
            // This handles the case where native uses Arrayed with repeated equations
            // while xmutil uses ApplyToAll
            if !elements.is_empty() {
                let first_expr = &elements[0].1;
                let all_same = elements
                    .iter()
                    .all(|(_, e, _, gf)| e == first_expr && gf.is_none());
                if all_same {
                    *eq = Equation::ApplyToAll(dims.clone(), first_expr.clone(), None);
                }
            }
        }
    }
}

fn normalize_stock(stock: &mut Stock) {
    // Canonicalize identifier
    stock.ident = canonical_ident(&stock.ident);
    // Normalize documentation
    stock.documentation = normalize_doc(&stock.documentation);
    // Normalize units
    stock.units = normalize_units(stock.units.as_ref());
    // Normalize equation
    normalize_equation(&mut stock.equation);
    // uid is view-related
    stock.uid = None;
    // AI state is not relevant to MDL parsing equivalence
    stock.ai_state = None;
    // Canonicalize inflows/outflows (ordering matches without sorting)
    stock.inflows = stock.inflows.iter().map(|s| canonical_ident(s)).collect();
    stock.outflows = stock.outflows.iter().map(|s| canonical_ident(s)).collect();
}

fn normalize_flow(flow: &mut Flow) {
    flow.ident = canonical_ident(&flow.ident);
    flow.documentation = normalize_doc(&flow.documentation);
    flow.units = normalize_units(flow.units.as_ref());
    normalize_equation(&mut flow.equation);
    // Graphical functions match without normalization
    flow.uid = None;
    flow.ai_state = None;
}

fn normalize_aux(aux: &mut Aux) {
    aux.ident = canonical_ident(&aux.ident);
    aux.documentation = normalize_doc(&aux.documentation);
    aux.units = normalize_units(aux.units.as_ref());
    normalize_equation(&mut aux.equation);
    // Graphical functions match without normalization
    aux.uid = None;
    aux.ai_state = None;
}

fn normalize_module(module: &mut Module) {
    module.ident = canonical_ident(&module.ident);
    module.documentation = normalize_doc(&module.documentation);
    module.units = normalize_units(module.units.as_ref());
    module.uid = None;
    module.ai_state = None;
}

/// Compare two projects for equivalence with detailed error messages.
fn assert_projects_equivalent(xmutil: &Project, native: &Project, path: &str) {
    // Compare sim_specs
    assert_eq!(
        xmutil.sim_specs, native.sim_specs,
        "{path}: sim_specs differ\n  xmutil: {:?}\n  native: {:?}",
        xmutil.sim_specs, native.sim_specs
    );

    // Compare dimensions
    assert_eq!(
        xmutil.dimensions.len(),
        native.dimensions.len(),
        "{path}: dimension count differs\n  xmutil ({} dims): {:?}\n  native ({} dims): {:?}",
        xmutil.dimensions.len(),
        xmutil
            .dimensions
            .iter()
            .map(|d| &d.name)
            .collect::<Vec<_>>(),
        native.dimensions.len(),
        native
            .dimensions
            .iter()
            .map(|d| &d.name)
            .collect::<Vec<_>>()
    );
    for (i, (xd, nd)) in xmutil
        .dimensions
        .iter()
        .zip(native.dimensions.iter())
        .enumerate()
    {
        assert_eq!(xd, nd, "{path}: dimension {i} differs");
    }

    // Compare unit equivalences
    assert_eq!(
        xmutil.units.len(),
        native.units.len(),
        "{path}: units count differs\n  xmutil ({} units): {:?}\n  native ({} units): {:?}",
        xmutil.units.len(),
        xmutil.units.iter().map(|u| &u.name).collect::<Vec<_>>(),
        native.units.len(),
        native.units.iter().map(|u| &u.name).collect::<Vec<_>>()
    );
    for (i, (xu, nu)) in xmutil.units.iter().zip(native.units.iter()).enumerate() {
        assert_eq!(xu, nu, "{path}: unit {i} ('{}') differs", xu.name);
    }

    // Compare models - use name-based matching instead of positional zip
    assert_eq!(
        xmutil.models.len(),
        native.models.len(),
        "{path}: model count differs"
    );

    // Build a map of native models by name for matching
    let native_models: HashMap<&str, &Model> =
        native.models.iter().map(|m| (m.name.as_str(), m)).collect();

    for xm in &xmutil.models {
        let nm = native_models.get(xm.name.as_str()).unwrap_or_else(|| {
            panic!(
                "{path}: model '{}' found in xmutil but not in native.\n  \
                 native models: {:?}",
                xm.name,
                native.models.iter().map(|m| &m.name).collect::<Vec<_>>()
            )
        });
        assert_model_equivalent(xm, nm, path);
    }
}

/// Compare two models for equivalence.
fn assert_model_equivalent(xm: &Model, nm: &Model, path: &str) {
    assert_eq!(
        xm.name, nm.name,
        "{path}: model name differs: '{}' vs '{}'",
        xm.name, nm.name
    );

    // Compare variable counts with helpful diff output
    if xm.variables.len() != nm.variables.len() {
        let xmutil_vars: Vec<_> = xm.variables.iter().map(|v| v.get_ident()).collect();
        let native_vars: Vec<_> = nm.variables.iter().map(|v| v.get_ident()).collect();

        // Find differences
        let only_in_xmutil: Vec<_> = xmutil_vars
            .iter()
            .filter(|v| !native_vars.contains(v))
            .collect();
        let only_in_native: Vec<_> = native_vars
            .iter()
            .filter(|v| !xmutil_vars.contains(v))
            .collect();

        panic!(
            "{path}: variable count differs ({} vs {})\n  \
             only in xmutil: {:?}\n  \
             only in native: {:?}",
            xm.variables.len(),
            nm.variables.len(),
            only_in_xmutil,
            only_in_native
        );
    }

    // Build a map of native variables by identifier for matching
    let native_vars: HashMap<&str, &Variable> =
        nm.variables.iter().map(|v| (v.get_ident(), v)).collect();

    // Compare each variable by name (not position)
    for xv in &xm.variables {
        let nv = native_vars.get(xv.get_ident()).unwrap_or_else(|| {
            panic!(
                "{path}: variable '{}' found in xmutil but not in native",
                xv.get_ident()
            )
        });
        if xv != *nv {
            panic!(
                "{path}: variable '{}' differs\n  xmutil: {:#?}\n  native: {:#?}",
                xv.get_ident(),
                xv,
                nv
            );
        }
    }
}

/// Test a single MDL file for equivalence between parsers.
fn test_single_mdl(mdl_path: &str) {
    let full_path = format!("../../{mdl_path}");
    eprintln!("testing: {mdl_path}");

    let content =
        fs::read_to_string(&full_path).unwrap_or_else(|e| panic!("Failed to read {mdl_path}: {e}"));

    // Parse via xmutil (MDL -> XMILE -> datamodel)
    let mut r1 = BufReader::new(content.as_bytes());
    let xmutil_project =
        open_vensim(&mut r1).unwrap_or_else(|e| panic!("{mdl_path}: xmutil failed: {e}"));

    // Parse via native (MDL -> datamodel directly)
    let mut r2 = BufReader::new(content.as_bytes());
    let native_project =
        open_vensim_native(&mut r2).unwrap_or_else(|e| panic!("{mdl_path}: native failed: {e}"));

    // Normalize both projects
    let xmutil_norm = normalize_project(xmutil_project);
    let native_norm = normalize_project(native_project);

    // Compare
    assert_projects_equivalent(&xmutil_norm, &native_norm, mdl_path);
}

#[test]
fn test_mdl_equivalence() {
    for &mdl_path in EQUIVALENT_MODELS {
        test_single_mdl(mdl_path);
    }
}
