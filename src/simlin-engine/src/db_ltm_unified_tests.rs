// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::datamodel;
use crate::test_common::TestProject;
use crate::testutils::{feedback_loop_project, x_aux, x_model};

#[test]
fn test_model_ltm_variables_generates_scores() {
    let db = SimlinDb::default();
    let project = feedback_loop_project();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let ltm = model_ltm_variables(&db, model, result.project);

    assert!(!ltm.vars.is_empty(), "should generate LTM variables");

    let has_link_score = ltm.vars.iter().any(|v| v.name.contains("link_score"));
    assert!(has_link_score, "should have link score variables");

    let has_loop_score = ltm.vars.iter().any(|v| v.name.contains("loop_score"));
    assert!(has_loop_score, "should have loop score variables");

    for var in &ltm.vars {
        assert!(
            !var.equation.is_empty(),
            "var {} should have non-empty equation",
            var.name
        );
    }
}

#[test]
fn test_model_ltm_variables_stdlib_module() {
    let db = SimlinDb::default();
    let stdlib_model = crate::stdlib::get("smth1").expect("smth1 stdlib model should exist");

    let project = datamodel::Project {
        name: "smth1_ltm_test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![stdlib_model],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["stdlib\u{205A}smth1"].source;

    let ltm = model_ltm_variables(&db, model, sync.project);

    let has_link_score = ltm.vars.iter().any(|v| v.name.contains("link_score"));
    assert!(has_link_score, "should have link score variables");

    let has_pathway = ltm.vars.iter().any(|v| v.name.contains("path"));
    assert!(has_pathway, "should have pathway variables");

    let has_composite = ltm.vars.iter().any(|v| v.name.contains("composite"));
    assert!(has_composite, "should have composite variables");

    let has_ilink = ltm.vars.iter().any(|v| v.name.contains("ilink"));
    assert!(
        !has_ilink,
        "no var name should contain 'ilink': {:?}",
        ltm.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
}

#[test]
fn test_model_ltm_variables_passthrough_module() {
    let db = SimlinDb::default();

    let project = datamodel::Project {
        name: "passthrough_test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![x_model(
            "passthrough",
            vec![
                x_aux("input", "0", None),
                x_aux("output", "input * 2", None),
            ],
        )],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["passthrough"].source;

    let ltm = model_ltm_variables(&db, model, sync.project);
    assert!(
        ltm.vars.is_empty(),
        "passthrough module with no stocks should produce no LTM vars"
    );
}

#[test]
fn test_model_ltm_variables_discovery_mode() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = feedback_loop_project();
    let (source_project, model) = {
        let result = sync_from_datamodel(&db, &project);
        (result.project, result.models["main"].source)
    };

    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let ltm = model_ltm_variables(&db, model, source_project);

    assert!(!ltm.vars.is_empty(), "should generate link score variables");

    let has_link_score = ltm.vars.iter().any(|v| v.name.contains("link_score"));
    assert!(has_link_score, "should have link score variables");

    let has_loop_score = ltm.vars.iter().any(|v| v.name.contains("loop_score"));
    assert!(
        !has_loop_score,
        "discovery mode should not have loop scores"
    );
}

/// Models with input ports that also have internal feedback loops should
/// get both pathway/composite scores AND loop/relative loop scores.
/// Regression test for a bug where has_input_ports caused loop score
/// generation to be skipped entirely.
#[test]
fn test_model_ltm_variables_input_ports_with_loops_get_loop_scores() {
    let db = SimlinDb::default();

    let stdlib_model = x_model(
        "main",
        vec![x_aux("x", "10", None), x_aux("s", "SMTH1(x, 5)", None)],
    );

    let project = datamodel::Project {
        name: "input_ports_loops_test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![stdlib_model],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["stdlib\u{205A}smth1"].source;

    let ltm = model_ltm_variables(&db, model, sync.project);

    let has_link_score = ltm.vars.iter().any(|v| v.name.contains("link_score"));
    assert!(has_link_score, "should have link score variables");

    let has_loop_score = ltm
        .vars
        .iter()
        .any(|v| v.name.contains("\u{205A}loop_score\u{205A}"));
    assert!(
        has_loop_score,
        "sub-model with feedback loops should have loop scores even when it has input ports: {:?}",
        ltm.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );

    let has_composite = ltm.vars.iter().any(|v| v.name.contains("composite"));
    assert!(has_composite, "should have composite variables");
}

/// Verify that model_ltm_variables sorts vars in dependency order:
/// link_scores first, then paths, then composites. This ensures the
/// VM evaluates them in the correct order since LTM vars are appended
/// to the flows runlist sequentially.
#[test]
fn test_model_ltm_variables_sort_order_respects_dependencies() {
    let db = SimlinDb::default();

    let stdlib_model = x_model(
        "main",
        vec![x_aux("x", "10", None), x_aux("s", "SMTH1(x, 5)", None)],
    );

    let project = datamodel::Project {
        name: "sort_order_test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![stdlib_model],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["stdlib\u{205A}smth1"].source;

    let ltm = model_ltm_variables(&db, model, sync.project);

    let mut last_category = 0u8;
    for var in &ltm.vars {
        let cat = if var.name.contains("\u{205A}composite\u{205A}") {
            3
        } else if var.name.contains("\u{205A}path\u{205A}") {
            2
        } else if var.name.contains("\u{205A}loop_score\u{205A}") {
            1
        } else {
            0
        };
        assert!(
            cat >= last_category,
            "LTM vars must be sorted in dependency order \
             (link_score < loop_score < path < composite), \
             but '{}' (category {}) follows category {}",
            var.name,
            cat,
            last_category
        );
        last_category = cat;
    }

    // Verify that all three categories are present
    assert!(
        ltm.vars.iter().any(|v| v.name.contains("link_score")),
        "should have link_score vars"
    );
    assert!(
        ltm.vars
            .iter()
            .any(|v| v.name.contains("\u{205A}path\u{205A}")),
        "should have path vars"
    );
    assert!(
        ltm.vars.iter().any(|v| v.name.contains("composite")),
        "should have composite vars"
    );
}

/// Verify that link scores for same-dimension A2A edges inherit the
/// target's dimension names.
#[test]
fn test_model_ltm_variables_a2a_same_dimension_link_scores() {
    use salsa::Setter;

    let project = TestProject::new("a2a_dims_test")
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_stock("population[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "population * 0.1", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let (source_project, model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    // Discovery mode generates link scores for ALL edges.
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let ltm = model_ltm_variables(&db, model, source_project);

    // Both population and births share [Region], so the link scores
    // for population->births and births->population should carry
    // the Region dimension.
    let link_scores: Vec<_> = ltm
        .vars
        .iter()
        .filter(|v| v.name.contains("link_score"))
        .collect();
    assert!(!link_scores.is_empty(), "should have link score variables");

    for ls in &link_scores {
        assert_eq!(
            ls.dimensions,
            vec!["Region".to_string()],
            "link score '{}' should have Region dimension, got {:?}",
            ls.name,
            ls.dimensions
        );
    }
}

/// Verify that scalar-to-arrayed edges produce link scores with the
/// target's dimensions.
#[test]
fn test_model_ltm_variables_scalar_to_arrayed_link_score() {
    use salsa::Setter;

    let project = TestProject::new("scalar_to_arrayed_test")
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_stock("population[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "population * growth_factor", None)
        .scalar_aux("growth_factor", "0.05")
        .build_datamodel();

    let mut db = SimlinDb::default();
    let (source_project, model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let ltm = model_ltm_variables(&db, model, source_project);

    // growth_factor is scalar, births is arrayed[Region].
    // The growth_factor->births link score should have Region dims.
    let gf_births_ls = ltm
        .vars
        .iter()
        .find(|v| {
            v.name.contains("link_score")
                && v.name.contains("growth_factor")
                && v.name.contains("births")
        })
        .expect("should have growth_factor->births link score");

    assert_eq!(
        gf_births_ls.dimensions,
        vec!["Region".to_string()],
        "scalar-to-arrayed link score should carry target's dimensions"
    );
}

/// Verify that scalar models still produce scalar link scores
/// (empty dimensions).
#[test]
fn test_model_ltm_variables_scalar_link_scores_have_empty_dimensions() {
    let db = SimlinDb::default();
    let project = feedback_loop_project();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let ltm = model_ltm_variables(&db, model, result.project);

    let link_scores: Vec<_> = ltm
        .vars
        .iter()
        .filter(|v| v.name.contains("link_score"))
        .collect();
    assert!(!link_scores.is_empty(), "should have link score variables");

    for ls in &link_scores {
        assert!(
            ls.dimensions.is_empty(),
            "scalar model link score '{}' should have empty dimensions, got {:?}",
            ls.name,
            ls.dimensions
        );
    }
}

/// Build a scalar project whose element-level causal graph is a single
/// cycle of `total_nodes` nodes (1 stock + 1 flow + (total_nodes - 2)
/// auxiliary variables).  Used by the auto-flip tests below.
///
/// Chain: `cap_stock -> aux_{N-3} -> aux_{N-4} -> ... -> aux_0 ->
/// cap_flow -> cap_stock`.  `cap_flow` is `cap_stock`'s only inflow, so
/// the flow-to-stock edge closes the cycle.  Every node lives in a
/// single `total_nodes`-sized SCC.
fn build_chain_scc_project(project_name: &str, total_nodes: usize) -> datamodel::Project {
    assert!(
        total_nodes >= 3,
        "chain SCC needs >= 3 nodes (stock + flow + >=1 aux), got {total_nodes}"
    );

    let aux_count = total_nodes - 2;
    let mut builder = crate::test_common::TestProject::new(project_name);
    for i in 0..aux_count {
        let name = format!("aux_{i}");
        let equation = if i + 1 == aux_count {
            "cap_stock".to_string()
        } else {
            format!("aux_{}", i + 1)
        };
        builder = builder.scalar_aux(&name, &equation);
    }
    builder = builder.flow("cap_flow", "aux_0", None);
    builder = builder.stock("cap_stock", "0", &["cap_flow"], &[], None);
    builder.build_datamodel()
}

/// Auto-flip: a model whose element-level causal graph has an SCC of
/// 51 nodes (one node over the 50-node threshold) must flip to
/// discovery-mode shape: link scores for causal edges, no per-loop
/// `loop_score` synthetic variables.  (`rel_loop_score` is never
/// materialized now that Option B moved it to post-sim.)
///
/// See `docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md` for the
/// compile-time equation-text blow-up that motivates this threshold.
#[test]
fn test_model_ltm_variables_auto_flip_above_scc_threshold() {
    let project = build_chain_scc_project("auto_flip_above", 51);
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;

    let ltm = model_ltm_variables(&db, model, sync.project);

    assert!(
        !ltm.vars.is_empty(),
        "auto-flipped LTM should still produce link score variables"
    );
    let has_link_score = ltm.vars.iter().any(|v| v.name.contains("link_score"));
    assert!(
        has_link_score,
        "auto-flipped LTM should have link score variables"
    );

    let loop_scores: Vec<_> = ltm
        .vars
        .iter()
        .filter(|v| v.name.contains("\u{205A}loop_score\u{205A}"))
        .collect();
    assert!(
        loop_scores.is_empty(),
        "auto-flipped LTM must NOT materialize loop_score vars; got: {:?}",
        loop_scores.iter().map(|v| &v.name).collect::<Vec<_>>()
    );

    // rel_loop_score is never materialized as a VM variable after Option B;
    // guard against any future regression that re-introduces the emitter.
    let rel_loop_scores: Vec<_> = ltm
        .vars
        .iter()
        .filter(|v| v.name.contains("\u{205A}rel_loop_score\u{205A}"))
        .collect();
    assert!(
        rel_loop_scores.is_empty(),
        "LTM must never materialize rel_loop_score vars (Option B); got: {:?}",
        rel_loop_scores.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
}

/// Counterpart: at 49 nodes (under the 50-node threshold) the
/// exhaustive path still runs and emits per-loop `loop_score` vars.
/// Guards against the threshold drifting too low and breaking LTM on
/// realistically sized models.
#[test]
fn test_model_ltm_variables_stays_exhaustive_below_scc_threshold() {
    let project = build_chain_scc_project("auto_flip_below", 49);
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;

    let ltm = model_ltm_variables(&db, model, sync.project);

    let has_loop_score = ltm
        .vars
        .iter()
        .any(|v| v.name.contains("\u{205A}loop_score\u{205A}"));
    assert!(
        has_loop_score,
        "below-threshold model should stay on the exhaustive path and emit \
         loop_score vars; got: {:?}",
        ltm.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
}

/// Auto-flip must surface a `CompilationDiagnostic::Warning` so the
/// caller can explain the mode change to the user.  The diagnostic is
/// accumulated by `model_ltm_variables` itself (not via
/// `model_all_diagnostics`), so we collect it directly from the
/// tracked function.
#[test]
fn test_model_ltm_variables_auto_flip_emits_warning_diagnostic() {
    use crate::db::{CompilationDiagnostic, DiagnosticError, DiagnosticSeverity};

    let project = build_chain_scc_project("auto_flip_diag", 51);
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;

    let _ = model_ltm_variables(&db, model, sync.project);

    let diags = model_ltm_variables::accumulated::<CompilationDiagnostic>(&db, model, sync.project);

    let has_auto_flip_warning = diags.iter().any(|CompilationDiagnostic(d)| {
        d.severity == DiagnosticSeverity::Warning
            && matches!(
                &d.error,
                DiagnosticError::Assembly(msg) if msg.contains("discovery mode")
            )
    });
    assert!(
        has_auto_flip_warning,
        "auto-flip should emit a Warning diagnostic mentioning 'discovery mode'; got: {:?}",
        diags.iter().map(|c| &c.0).collect::<Vec<_>>()
    );
}

/// The auto-flip warning must also surface through the diagnostic
/// collector that both `libsimlin` and `simlin-mcp` use to hand
/// diagnostics to end users.  Accumulation on `model_ltm_variables`
/// alone is not enough -- `model_all_diagnostics` must drive LTM
/// synthesis when `ltm_enabled` so salsa's accumulator propagates the
/// warning to the collector.  Without this guarantee, the auto-flip is
/// silent from the user's perspective.
///
/// `collect_all_diagnostics` is a trivial wrapper over
/// `collect_model_diagnostics`; we assert on the per-model collector
/// here to sidestep `SyncResult`'s db borrow.
#[test]
fn test_auto_flip_warning_surfaces_via_collect_model_diagnostics() {
    use crate::db::{DiagnosticError, DiagnosticSeverity, collect_model_diagnostics};
    use salsa::Setter;

    let project = build_chain_scc_project("auto_flip_surface", 51);
    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let diags = collect_model_diagnostics(&db, source_model, source_project);

    let has_auto_flip_warning = diags.iter().any(|d| {
        d.severity == DiagnosticSeverity::Warning
            && matches!(
                &d.error,
                DiagnosticError::Assembly(msg) if msg.contains("discovery mode")
            )
    });
    assert!(
        has_auto_flip_warning,
        "auto-flip warning must reach collect_model_diagnostics; got: {:?}",
        diags
    );
}

/// Counterpart to the surfacing test: when LTM is disabled,
/// `collect_model_diagnostics` must not run LTM synthesis -- a silently
/// auto-flipping model whose caller never asked for LTM should not emit
/// LTM diagnostics.
#[test]
fn test_ltm_disabled_does_not_surface_auto_flip_warning() {
    use crate::db::{DiagnosticError, DiagnosticSeverity, collect_model_diagnostics};

    let project = build_chain_scc_project("auto_flip_disabled", 51);
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;

    assert!(
        !sync.project.ltm_enabled(&db),
        "baseline: ltm_enabled must default to false"
    );

    let diags = collect_model_diagnostics(&db, source_model, sync.project);

    let has_auto_flip_warning = diags.iter().any(|d| {
        d.severity == DiagnosticSeverity::Warning
            && matches!(
                &d.error,
                DiagnosticError::Assembly(msg) if msg.contains("discovery mode")
            )
    });
    assert!(
        !has_auto_flip_warning,
        "LTM-disabled project must not emit LTM diagnostics; got: {:?}",
        diags
    );
}
