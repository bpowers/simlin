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

use simlin_compat::{open_vensim, open_vensim_native};
use simlin_core::canonicalize;
use simlin_core::datamodel::{
    Aux, Dimension, DimensionElements, Dt, Equation, Flow, Model, Module, Project, SimSpecs, Stock,
    Variable, View, ViewElement, view_element,
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

/// Summarize view elements for debugging.
fn summarize_view_elements(views: &[View]) -> String {
    if views.is_empty() {
        return "no views".to_string();
    }
    let View::StockFlow(sf) = &views[0];
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for elem in &sf.elements {
        let kind = match elem {
            ViewElement::Aux(_) => "aux",
            ViewElement::Stock(_) => "stock",
            ViewElement::Flow(_) => "flow",
            ViewElement::Link(_) => "link",
            ViewElement::Module(_) => "module",
            ViewElement::Alias(_) => "alias",
            ViewElement::Cloud(_) => "cloud",
            ViewElement::Group(_) => "group",
        };
        *counts.entry(kind).or_insert(0) += 1;
    }
    let mut parts: Vec<_> = counts.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
    parts.sort();
    format!("{} elements ({})", sf.elements.len(), parts.join(", "))
}

/// Normalize views for comparison.
/// TODO: Full view comparison once all differences are understood.
fn normalize_views(views: &mut [View]) {
    if !views.is_empty() {
        eprintln!("  {}", summarize_view_elements(views));
    }
}

/// Normalize a model for comparison.
fn normalize_model(model: &mut Model) {
    // Normalize views for comparison
    normalize_views(&mut model.views);

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
        Equation::Scalar(expr, initial_comment) => {
            *expr = normalize_expr(expr);
            // The initial-value comment is an expression string (e.g. from ACTIVE INITIAL),
            // not documentation, so normalize it as an expression.
            if let Some(c) = initial_comment {
                *c = normalize_expr(c);
            }
        }
        Equation::ApplyToAll(dims, expr, initial_comment) => {
            // Lowercase and sort dimension names for consistent comparison
            for dim in dims.iter_mut() {
                *dim = dim.to_lowercase();
            }
            dims.sort();
            *expr = normalize_expr(expr);
            if let Some(c) = initial_comment {
                *c = normalize_expr(c);
            }
        }
        Equation::Arrayed(dims, elements) => {
            // Lowercase and sort dimension names for consistent comparison
            for dim in dims.iter_mut() {
                *dim = dim.to_lowercase();
            }
            dims.sort();
            // Normalize each element
            for (subscript, expr, initial_comment, gf) in elements.iter_mut() {
                // Canonicalize subscript names
                *subscript = canonical_ident(subscript);
                *expr = normalize_expr(expr);
                if let Some(c) = initial_comment {
                    *c = normalize_expr(c);
                }
                // Graphical functions match without normalization
                let _ = gf;
            }
            // Sort elements by canonicalized subscript key for order-independent
            // comparison. The native parser and xmutil may produce different
            // element orderings; this is not a semantic difference.
            elements.sort_by(|a, b| a.0.cmp(&b.0));

            // Check if all elements have the same expression - if so, convert to ApplyToAll
            // This handles the case where native uses Arrayed with repeated equations
            // while xmutil uses ApplyToAll
            if !elements.is_empty() {
                let first_expr = &elements[0].1;
                let first_initial = &elements[0].2;
                let all_same = elements.iter().all(|(_, e, init, gf)| {
                    e == first_expr && init == first_initial && gf.is_none()
                });
                if all_same {
                    *eq = Equation::ApplyToAll(
                        dims.clone(),
                        first_expr.clone(),
                        first_initial.clone(),
                    );
                }
            }
        }
    }
}

#[test]
fn test_normalize_arrayed_to_apply_to_all_preserves_initial() {
    let mut eq = Equation::Arrayed(
        vec!["dim".to_string()],
        vec![
            (
                "a".to_string(),
                "x+1".to_string(),
                Some("INIT_VAL".to_string()),
                None,
            ),
            (
                "b".to_string(),
                "x+1".to_string(),
                Some("INIT_VAL".to_string()),
                None,
            ),
        ],
    );
    normalize_equation(&mut eq);
    // After normalization, expressions (including initial-value comments) are lowercased
    // and spaces around operators are removed.
    assert_eq!(
        eq,
        Equation::ApplyToAll(
            vec!["dim".to_string()],
            "x+1".to_string(),
            Some("init_val".to_string())
        )
    );
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

/// Floating-point tolerance for arc angles.
/// The xmutil path roundtrips angles through a string representation in XMILE XML
/// (C++ float → string → Rust f64), while the native path computes directly.
/// This causes ~1e-13 differences that are inherent to the different computation paths.
const ANGLE_EPSILON: f64 = 1e-10;

/// Compare two LinkShapes with floating-point tolerance for arc angles.
fn link_shapes_equivalent(a: &view_element::LinkShape, b: &view_element::LinkShape) -> bool {
    match (a, b) {
        (view_element::LinkShape::Straight, view_element::LinkShape::Straight) => true,
        (view_element::LinkShape::Arc(a), view_element::LinkShape::Arc(b)) => {
            (a - b).abs() < ANGLE_EPSILON
        }
        (view_element::LinkShape::MultiPoint(a), view_element::LinkShape::MultiPoint(b)) => a == b,
        _ => false,
    }
}

/// Compare two ViewElements with tolerance for floating-point differences.
fn assert_view_elements_equivalent(
    xe: &ViewElement,
    ne: &ViewElement,
    path: &str,
    view_idx: usize,
    elem_idx: usize,
) {
    // For Link elements, use fuzzy comparison for arc angles
    if let (ViewElement::Link(xl), ViewElement::Link(nl)) = (xe, ne) {
        assert_eq!(
            xl.uid, nl.uid,
            "{path}: view {view_idx} element {elem_idx} link uid differs"
        );
        assert_eq!(
            xl.from_uid, nl.from_uid,
            "{path}: view {view_idx} element {elem_idx} link from_uid differs"
        );
        assert_eq!(
            xl.to_uid, nl.to_uid,
            "{path}: view {view_idx} element {elem_idx} link to_uid differs"
        );
        assert!(
            link_shapes_equivalent(&xl.shape, &nl.shape),
            "{path}: view {view_idx} element {elem_idx} link shape differs\n  xmutil: {:?}\n  native: {:?}",
            xl.shape,
            nl.shape
        );
        assert_eq!(
            xl.polarity, nl.polarity,
            "{path}: view {view_idx} element {elem_idx} link polarity differs"
        );
        return;
    }
    // For all other elements, exact equality
    assert_eq!(
        xe, ne,
        "{path}: view {view_idx} element {elem_idx} differs\n  xmutil: {:#?}\n  native: {:#?}",
        xe, ne
    );
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

    // Compare views
    assert_eq!(
        xm.views.len(),
        nm.views.len(),
        "{path}: view count differs ({} vs {})",
        xm.views.len(),
        nm.views.len()
    );
    for (i, (xv, nv)) in xm.views.iter().zip(nm.views.iter()).enumerate() {
        let View::StockFlow(x_sf) = xv;
        let View::StockFlow(n_sf) = nv;
        assert_eq!(
            x_sf.elements.len(),
            n_sf.elements.len(),
            "{path}: view {i} element count differs ({} vs {})\n  xmutil: {}\n  native: {}",
            x_sf.elements.len(),
            n_sf.elements.len(),
            summarize_view_elements(&xm.views),
            summarize_view_elements(&nm.views),
        );
        for (j, (xe, ne)) in x_sf.elements.iter().zip(n_sf.elements.iter()).enumerate() {
            assert_view_elements_equivalent(xe, ne, path, i, j);
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
    let xmutil_project =
        open_vensim(&content).unwrap_or_else(|e| panic!("{mdl_path}: xmutil failed: {e}"));

    // Parse via native (MDL -> datamodel directly)
    let native_project =
        open_vensim_native(&content).unwrap_or_else(|e| panic!("{mdl_path}: native failed: {e}"));

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

/// Standalone test for the C-LEARN model which causes a segfault in xmutil's C++ code.
#[test]
fn test_clearn_xmutil() {
    let mdl_path = "test/xmutil_test_models/C-LEARN v77 for Vensim.mdl";
    let full_path = format!("../../{mdl_path}");

    let content =
        fs::read_to_string(&full_path).unwrap_or_else(|e| panic!("Failed to read {mdl_path}: {e}"));

    // Parse via xmutil (MDL -> XMILE -> datamodel) -- this segfaults in xmutil's C++ code
    let _project =
        open_vensim(&content).unwrap_or_else(|e| panic!("{mdl_path}: xmutil failed: {e}"));
}

/// Verify the native Rust parser can handle the C-LEARN model (no xmutil dependency).
#[test]
fn test_clearn_native() {
    let mdl_path = "test/xmutil_test_models/C-LEARN v77 for Vensim.mdl";
    let full_path = format!("../../{mdl_path}");

    let content =
        fs::read_to_string(&full_path).unwrap_or_else(|e| panic!("Failed to read {mdl_path}: {e}"));

    // Parse via native (MDL -> datamodel directly)
    let _project =
        open_vensim_native(&content).unwrap_or_else(|e| panic!("{mdl_path}: native failed: {e}"));
}

// ===== Non-panicking comparison infrastructure =====
//
// These functions collect all differences into a Vec<String> instead of
// panicking on the first mismatch.  This lets a single test run surface
// every class of problem at once.

/// Describes one field-level difference between the xmutil and native output.
struct Diff {
    /// Dot-path to the differing field, e.g. "model[main].var[population].equation"
    path: String,
    detail: String,
}

impl std::fmt::Display for Diff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "  {}: {}", self.path, self.detail)
    }
}

/// Collect all differences between two normalized projects.
fn collect_project_diffs(xmutil: &Project, native: &Project, path: &str) -> Vec<Diff> {
    let mut diffs = Vec::new();

    // sim_specs
    if xmutil.sim_specs != native.sim_specs {
        diffs.push(Diff {
            path: format!("{path}.sim_specs"),
            detail: format!(
                "xmutil: {:?}\nnative: {:?}",
                xmutil.sim_specs, native.sim_specs
            ),
        });
    }

    // dimensions
    collect_dimension_diffs(&mut diffs, xmutil, native, path);

    // unit equivalences
    collect_unit_diffs(&mut diffs, xmutil, native, path);

    // models
    collect_model_diffs(&mut diffs, xmutil, native, path);

    diffs
}

fn collect_dimension_diffs(diffs: &mut Vec<Diff>, xmutil: &Project, native: &Project, path: &str) {
    let x_by_name: HashMap<&str, &Dimension> = xmutil
        .dimensions
        .iter()
        .map(|d| (d.name.as_str(), d))
        .collect();
    let n_by_name: HashMap<&str, &Dimension> = native
        .dimensions
        .iter()
        .map(|d| (d.name.as_str(), d))
        .collect();

    for (name, xd) in &x_by_name {
        match n_by_name.get(name) {
            None => diffs.push(Diff {
                path: format!("{path}.dim[{name}]"),
                detail: "only in xmutil".into(),
            }),
            Some(nd) if *xd != *nd => diffs.push(Diff {
                path: format!("{path}.dim[{name}]"),
                detail: format!("xmutil: {:?}\nnative: {:?}", xd, nd),
            }),
            _ => {}
        }
    }
    for name in n_by_name.keys() {
        if !x_by_name.contains_key(name) {
            diffs.push(Diff {
                path: format!("{path}.dim[{name}]"),
                detail: "only in native".into(),
            });
        }
    }
}

fn collect_unit_diffs(diffs: &mut Vec<Diff>, xmutil: &Project, native: &Project, path: &str) {
    let x_by_name: HashMap<&str, _> = xmutil.units.iter().map(|u| (u.name.as_str(), u)).collect();
    let n_by_name: HashMap<&str, _> = native.units.iter().map(|u| (u.name.as_str(), u)).collect();

    for (name, xu) in &x_by_name {
        match n_by_name.get(name) {
            None => diffs.push(Diff {
                path: format!("{path}.unit[{name}]"),
                detail: "only in xmutil".into(),
            }),
            Some(nu) if *xu != *nu => diffs.push(Diff {
                path: format!("{path}.unit[{name}]"),
                detail: format!("xmutil: {:?}\nnative: {:?}", xu, nu),
            }),
            _ => {}
        }
    }
    for name in n_by_name.keys() {
        if !x_by_name.contains_key(name) {
            diffs.push(Diff {
                path: format!("{path}.unit[{name}]"),
                detail: "only in native".into(),
            });
        }
    }
}

fn collect_model_diffs(diffs: &mut Vec<Diff>, xmutil: &Project, native: &Project, path: &str) {
    let x_by_name: HashMap<&str, &Model> =
        xmutil.models.iter().map(|m| (m.name.as_str(), m)).collect();
    let n_by_name: HashMap<&str, &Model> =
        native.models.iter().map(|m| (m.name.as_str(), m)).collect();

    for (name, xm) in &x_by_name {
        match n_by_name.get(name) {
            None => diffs.push(Diff {
                path: format!("{path}.model[{name}]"),
                detail: "only in xmutil".into(),
            }),
            Some(nm) => collect_single_model_diffs(diffs, xm, nm, &format!("{path}.model[{name}]")),
        }
    }
    for name in n_by_name.keys() {
        if !x_by_name.contains_key(name) {
            diffs.push(Diff {
                path: format!("{path}.model[{name}]"),
                detail: "only in native".into(),
            });
        }
    }
}

fn collect_single_model_diffs(diffs: &mut Vec<Diff>, xm: &Model, nm: &Model, path: &str) {
    let x_by_ident: HashMap<&str, &Variable> =
        xm.variables.iter().map(|v| (v.get_ident(), v)).collect();
    let n_by_ident: HashMap<&str, &Variable> =
        nm.variables.iter().map(|v| (v.get_ident(), v)).collect();

    // Variables only in xmutil
    let mut only_xmutil: Vec<&str> = x_by_ident
        .keys()
        .filter(|k| !n_by_ident.contains_key(*k))
        .copied()
        .collect();
    only_xmutil.sort();
    for ident in &only_xmutil {
        diffs.push(Diff {
            path: format!("{path}.var[{ident}]"),
            detail: "only in xmutil".into(),
        });
    }

    // Variables only in native
    let mut only_native: Vec<&str> = n_by_ident
        .keys()
        .filter(|k| !x_by_ident.contains_key(*k))
        .copied()
        .collect();
    only_native.sort();
    for ident in &only_native {
        diffs.push(Diff {
            path: format!("{path}.var[{ident}]"),
            detail: "only in native".into(),
        });
    }

    // Variables present in both but differing
    let mut common: Vec<&str> = x_by_ident
        .keys()
        .filter(|k| n_by_ident.contains_key(*k))
        .copied()
        .collect();
    common.sort();
    for ident in common {
        let xv = x_by_ident[ident];
        let nv = n_by_ident[ident];
        if xv != nv {
            collect_variable_field_diffs(diffs, xv, nv, &format!("{path}.var[{ident}]"));
        }
    }
}

/// Compare individual fields of two variables to pinpoint exactly what differs.
fn collect_variable_field_diffs(diffs: &mut Vec<Diff>, xv: &Variable, nv: &Variable, path: &str) {
    // Type mismatch (Stock vs Flow vs Aux vs Module)
    let xtype = match xv {
        Variable::Stock(_) => "Stock",
        Variable::Flow(_) => "Flow",
        Variable::Aux(_) => "Aux",
        Variable::Module(_) => "Module",
    };
    let ntype = match nv {
        Variable::Stock(_) => "Stock",
        Variable::Flow(_) => "Flow",
        Variable::Aux(_) => "Aux",
        Variable::Module(_) => "Module",
    };
    if xtype != ntype {
        diffs.push(Diff {
            path: format!("{path}.type"),
            detail: format!("xmutil: {xtype}, native: {ntype}"),
        });
        return;
    }

    // Compare fields depending on type
    match (xv, nv) {
        (Variable::Stock(xs), Variable::Stock(ns)) => {
            diff_field(diffs, path, "equation", &xs.equation, &ns.equation);
            diff_field(
                diffs,
                path,
                "documentation",
                &xs.documentation,
                &ns.documentation,
            );
            diff_field(diffs, path, "units", &xs.units, &ns.units);
            diff_field(diffs, path, "inflows", &xs.inflows, &ns.inflows);
            diff_field(diffs, path, "outflows", &xs.outflows, &ns.outflows);
            diff_field(
                diffs,
                path,
                "non_negative",
                &xs.non_negative,
                &ns.non_negative,
            );
        }
        (Variable::Flow(xf), Variable::Flow(nf)) => {
            diff_field(diffs, path, "equation", &xf.equation, &nf.equation);
            diff_field(
                diffs,
                path,
                "documentation",
                &xf.documentation,
                &nf.documentation,
            );
            diff_field(diffs, path, "units", &xf.units, &nf.units);
            diff_field(diffs, path, "gf", &xf.gf, &nf.gf);
            diff_field(
                diffs,
                path,
                "non_negative",
                &xf.non_negative,
                &nf.non_negative,
            );
        }
        (Variable::Aux(xa), Variable::Aux(na)) => {
            diff_field(diffs, path, "equation", &xa.equation, &na.equation);
            diff_field(
                diffs,
                path,
                "documentation",
                &xa.documentation,
                &na.documentation,
            );
            diff_field(diffs, path, "units", &xa.units, &na.units);
            diff_field(diffs, path, "gf", &xa.gf, &na.gf);
        }
        (Variable::Module(xmod), Variable::Module(nmod)) => {
            diff_field(
                diffs,
                path,
                "documentation",
                &xmod.documentation,
                &nmod.documentation,
            );
            diff_field(diffs, path, "units", &xmod.units, &nmod.units);
            diff_field(
                diffs,
                path,
                "references",
                &xmod.references,
                &nmod.references,
            );
        }
        _ => unreachable!(),
    }
}

fn diff_field<T: std::fmt::Debug + PartialEq>(
    diffs: &mut Vec<Diff>,
    path: &str,
    field: &str,
    xval: &T,
    nval: &T,
) {
    if xval != nval {
        diffs.push(Diff {
            path: format!("{path}.{field}"),
            detail: format!("xmutil: {:?}\nnative: {:?}", xval, nval),
        });
    }
}

/// Compare the xmutil and native parser outputs for the C-LEARN model,
/// collecting all differences rather than stopping at the first.
#[test]
#[ignore]
fn test_clearn_equivalence() {
    let mdl_path = "test/xmutil_test_models/C-LEARN v77 for Vensim.mdl";
    let full_path = format!("../../{mdl_path}");

    let content =
        fs::read_to_string(&full_path).unwrap_or_else(|e| panic!("Failed to read {mdl_path}: {e}"));

    let xmutil_project =
        open_vensim(&content).unwrap_or_else(|e| panic!("{mdl_path}: xmutil failed: {e}"));

    let native_project =
        open_vensim_native(&content).unwrap_or_else(|e| panic!("{mdl_path}: native failed: {e}"));

    let xmutil_norm = normalize_project(xmutil_project);
    let native_norm = normalize_project(native_project);

    let diffs = collect_project_diffs(&xmutil_norm, &native_norm, mdl_path);

    if !diffs.is_empty() {
        let mut report = format!("\n{} equivalence differences found:\n", diffs.len());
        for d in &diffs {
            report.push_str(&format!("{d}\n"));
        }
        panic!("{report}");
    }
}

#[test]
fn test_normalize_equation_sorts_arrayed_elements() {
    // Arrayed equations with identical elements in different orders should be
    // equal after normalization. This verifies that the element sort in
    // normalize_equation works correctly.
    let mut eq_a = Equation::Arrayed(
        vec!["dim".to_string()],
        vec![
            ("b".to_string(), "1".to_string(), None, None),
            ("a".to_string(), "2".to_string(), None, None),
            ("c".to_string(), "3".to_string(), None, None),
        ],
    );
    let mut eq_b = Equation::Arrayed(
        vec!["dim".to_string()],
        vec![
            ("a".to_string(), "2".to_string(), None, None),
            ("c".to_string(), "3".to_string(), None, None),
            ("b".to_string(), "1".to_string(), None, None),
        ],
    );

    normalize_equation(&mut eq_a);
    normalize_equation(&mut eq_b);

    assert_eq!(
        eq_a, eq_b,
        "Arrayed equations with different element order should be equal after normalization"
    );
}

#[test]
fn test_normalize_equation_sorts_multidim_arrayed_elements() {
    // Multi-dimensional arrayed equations with comma-separated subscript keys
    // should also sort correctly.
    let mut eq_a = Equation::Arrayed(
        vec!["dima".to_string(), "dimb".to_string()],
        vec![
            ("b,y".to_string(), "3".to_string(), None, None),
            ("a,x".to_string(), "1".to_string(), None, None),
            ("a,y".to_string(), "2".to_string(), None, None),
            ("b,x".to_string(), "4".to_string(), None, None),
        ],
    );
    let mut eq_b = Equation::Arrayed(
        vec!["dima".to_string(), "dimb".to_string()],
        vec![
            ("a,x".to_string(), "1".to_string(), None, None),
            ("a,y".to_string(), "2".to_string(), None, None),
            ("b,x".to_string(), "4".to_string(), None, None),
            ("b,y".to_string(), "3".to_string(), None, None),
        ],
    );

    normalize_equation(&mut eq_a);
    normalize_equation(&mut eq_b);

    assert_eq!(
        eq_a, eq_b,
        "Multi-dim arrayed equations should be equal after normalization"
    );
}
