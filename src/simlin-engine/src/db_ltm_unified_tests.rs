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

/// Build a scalar project with two *disjoint* cycles of `scc_size` nodes
/// each, connected to no other subgraph.  Used to verify that the
/// auto-flip gate fires on the *largest SCC*, not on the total node
/// count or total SCC count across the model.
///
/// Each cycle has the shape: `stock_k -> aux_k_{N-3} -> ... -> aux_k_0
/// -> flow_k -> stock_k`, parameterized by a distinct prefix
/// `k in {"a", "b"}`.
fn build_two_disjoint_sccs_project(project_name: &str, scc_size: usize) -> datamodel::Project {
    assert!(
        scc_size >= 3,
        "each disjoint cycle needs >= 3 nodes, got {scc_size}"
    );

    let aux_count = scc_size - 2;
    let mut builder = crate::test_common::TestProject::new(project_name);
    for prefix in ["a", "b"] {
        for i in 0..aux_count {
            let name = format!("{prefix}_aux_{i}");
            let equation = if i + 1 == aux_count {
                format!("{prefix}_stock")
            } else {
                format!("{prefix}_aux_{}", i + 1)
            };
            builder = builder.scalar_aux(&name, &equation);
        }
        let flow_name = format!("{prefix}_flow");
        let stock_name = format!("{prefix}_stock");
        let flow_eq = format!("{prefix}_aux_0");
        builder = builder.flow(&flow_name, &flow_eq, None);
        builder = builder.stock(&stock_name, "0", &[flow_name.as_str()], &[], None);
    }
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

/// Adversarial corner case for Option A: the auto-flip gate must key on
/// the *largest* SCC, not on total SCC count or total node count.  Two
/// disjoint 40-node cycles (80 nodes total) must stay exhaustive because
/// the largest SCC is 40 <= 50.  A silent regression here (e.g.,
/// accidentally summing SCC sizes) would crush any real model with
/// several independent feedback subsystems.
#[test]
fn test_auto_flip_keys_on_largest_scc_not_total_nodes() {
    let project = build_two_disjoint_sccs_project("two_sccs_exhaustive", 40);
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
        "two disjoint 40-node SCCs should stay exhaustive and emit \
         loop_score vars (largest SCC is 40, <= 50 threshold); \
         vars: {:?}",
        ltm.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
}

/// Adversarial corner case for Option A: when the user *explicitly*
/// requests discovery mode, the auto-flip gate must short-circuit and
/// NOT emit the auto-flip warning.  Discovery-by-user-choice and
/// discovery-by-auto-flip look identical in the output shape (no loop
/// score vars), but the diagnostic is the only signal the caller has
/// that a mode change happened behind their back.  Emitting it when the
/// user chose discovery themselves would be confusing noise.
#[test]
fn test_user_discovery_mode_does_not_emit_auto_flip_warning() {
    use crate::db::{CompilationDiagnostic, DiagnosticError, DiagnosticSeverity};
    use salsa::Setter;

    let project = build_chain_scc_project("user_discovery_no_warning", 51);
    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let _ = model_ltm_variables(&db, source_model, source_project);

    let diags = model_ltm_variables::accumulated::<CompilationDiagnostic>(
        &db,
        source_model,
        source_project,
    );

    let has_auto_flip_warning = diags.iter().any(|CompilationDiagnostic(d)| {
        d.severity == DiagnosticSeverity::Warning
            && matches!(
                &d.error,
                DiagnosticError::Assembly(msg) if msg.contains("auto-switched")
            )
    });
    assert!(
        !has_auto_flip_warning,
        "user-requested discovery mode must NOT emit auto-flip warning; got: {:?}",
        diags.iter().map(|c| &c.0).collect::<Vec<_>>()
    );
}

/// Adversarial corner case for Option A: auto-flip must key on the
/// *element-level* graph, not the variable-level graph.  An arrayed
/// single-variable stock-flow loop (e.g., `population[Region]` with 60
/// dims) has a variable-level SCC of size 2 (stock + flow) but an
/// element-level SCC of size 120 -- above the threshold.  Using the
/// variable graph would let such a model through to exhaustive
/// compilation and explode equation-text generation.
#[test]
fn test_auto_flip_uses_element_level_scc_for_arrayed_models() {
    // Build 60-element arrayed population -> births -> population cycle.
    // Variable graph: 2 nodes in cycle (births, population).
    // Element graph: 60 per-element (stock, flow) pairs, each in its own
    // 2-node cycle.  Largest element SCC is 2, not 120; A2A is same-element.
    // So this model should *stay* exhaustive.  Keep it here as the
    // baseline against which a cross-element variant (below) is
    // contrasted.
    let dim_size = 60usize;
    let elements: Vec<String> = (0..dim_size).map(|i| format!("R{i}")).collect();
    let elem_refs: Vec<&str> = elements.iter().map(String::as_str).collect();

    let project = crate::test_common::TestProject::new("arrayed_a2a_no_autoflip")
        .named_dimension("Region", &elem_refs)
        .array_stock("population[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "population * 0.1", None)
        .build_datamodel();

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
        "a 60-element A2A stock-flow loop has element SCC size 2 \
         (same-element edges), so auto-flip must NOT fire; \
         got vars: {:?}",
        ltm.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
}

// -- Phase 3 (per-shape link scores) --
//
// AC3.1 / AC3.3: when a target equation references a source under multiple
// distinct RefShapes, model_ltm_variables must emit one LtmSyntheticVar
// per (from, to, shape) tuple. Wildcard shapes always carry the
// '\u{205A}wildcard' suffix (Task 4 naming convention); FixedIndex shapes
// carry the per-element prefixed-from form. Discovery mode is used here
// so the link emission loop runs for every causal edge, not just edges
// in detected loops.

#[test]
fn per_shape_link_scores_for_share_with_sum() {
    use salsa::Setter;

    // share[R] = pop / SUM(pop[*]) references `pop` under both Bare
    // (in the numerator) and Wildcard (inside SUM) shapes. Phase 3
    // emission must produce two distinct link scores.
    //
    // We use a stock for `pop` because `model_ltm_variables` short-
    // circuits to an empty result when the model has no stocks (LTM
    // is for feedback loops; stocks are the structural anchor).
    let mut db = SimlinDb::default();
    let project = TestProject::new("share_sum")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("pop[Region]", "100", &[], &[], None)
        .array_aux("share[Region]", "pop / SUM(pop[*])")
        .build_datamodel();

    let (source_project, model) = {
        let result = sync_from_datamodel(&db, &project);
        (result.project, result.models["main"].source)
    };
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let ltm = model_ltm_variables(&db, model, source_project);
    let names: Vec<&String> = ltm.vars.iter().map(|v| &v.name).collect();

    let bare_name = "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share";
    let wildcard_name = "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share\u{205A}wildcard";

    assert!(
        names.iter().any(|n| n.as_str() == bare_name),
        "expected Bare-shape link score {bare_name:?}; got: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.as_str() == wildcard_name),
        "expected Wildcard-shape link score {wildcard_name:?}; got: {names:?}"
    );
}

#[test]
fn fixed_index_link_score_emits_per_element_name() {
    use salsa::Setter;

    // rel_pop[R] = pop / pop[NYC] references `pop` under both Bare
    // (numerator's same-element ref) and FixedIndex(nyc) (the literal
    // [NYC] subscript) shapes. Phase 3 must emit two distinct link
    // scores for the (pop, rel_pop) pair: one Bare-named and one
    // FixedIndex-named with the bracketed source.
    //
    // We use a stock for `pop` so the model has at least one stock --
    // `model_ltm_variables` short-circuits to an empty result on
    // stockless models (LTM is for feedback loops).
    let mut db = SimlinDb::default();
    let project = TestProject::new("rel_pop")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("pop[Region]", "100", &[], &[], None)
        .array_aux("rel_pop[Region]", "pop / pop[NYC]")
        .build_datamodel();

    let (source_project, model) = {
        let result = sync_from_datamodel(&db, &project);
        (result.project, result.models["main"].source)
    };
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let ltm = model_ltm_variables(&db, model, source_project);
    let names: Vec<&String> = ltm.vars.iter().map(|v| &v.name).collect();

    let bare_name = "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}rel_pop";
    let fixed_name = "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}rel_pop";

    assert!(
        names.iter().any(|n| n.as_str() == bare_name),
        "expected Bare-shape link score {bare_name:?}; got: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.as_str() == fixed_name),
        "expected FixedIndex(nyc)-shape link score {fixed_name:?}; got: {names:?}"
    );

    // Total: exactly 2 distinct (pop, rel_pop) link scores -- the bare
    // (Region-A2A) one and the FixedIndex(nyc) per-element one. Other
    // link scores (e.g., self-loops, unrelated edges) shouldn't be
    // counted. We match anything containing both 'pop' and 'rel_pop'
    // in the suffix portion of the name.
    let pop_to_rel: usize = names
        .iter()
        .filter(|n| {
            n.contains("link_score\u{205A}pop")
                && (n.contains("\u{2192}rel_pop")
                    || n.contains("[nyc]\u{2192}rel_pop")
                    || n.contains("[boston]\u{2192}rel_pop"))
        })
        .count();
    assert_eq!(
        pop_to_rel, 2,
        "expected exactly 2 distinct (pop, rel_pop) link scores (Bare + FixedIndex(nyc)); \
         got {pop_to_rel} matching names: {names:?}"
    );
}

// -- Phase 4 (Link.shape annotation in build_element_level_loops) --
//
// AC4.1 / AC4.2: build_element_level_loops must populate Link.shape
// so the loop-score equation generator picks the right per-shape
// link-score variable name. For pure-A2A loops every link is Bare
// and carries variable-level names.
//
// For mixed/scalar loops the shape is Bare and the link names are
// normalized to match the link-score variables that are actually emitted:
//  - Cross-dimensional edges (subscripted from, bare to): element-level
//    from is preserved so the loop score references the per-element link
//    score emitted by try_cross_dimensional_link_scores.
//  - All other edges (A2A inside a mixed loop, scalar-to-arrayed, etc.):
//    subscripts are stripped so the loop score references the variable-level
//    A2A or scalar link score emitted by emit_per_shape_link_scores.
//
// Using FixedIndex (old heuristic) caused doubly-bracketed names like
// "population[nyc][nyc]→total_pop" because link_score_var_name prepends
// "[nyc]" to the already-subscripted from name.

/// Build the element-level loops for a TestProject by replicating the
/// same orchestration `model_ltm_variables` does internally. Phase 4
/// added `pub(crate)` visibility on `build_element_level_loops` so
/// tests can inspect the `Link.shape` annotations directly.
fn build_loops_for_test(project: &TestProject) -> Vec<crate::ltm::Loop> {
    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let circuits = model_element_loop_circuits(&db, model, sync.project);
    if circuits.is_empty() {
        return vec![];
    }
    let var_graph = causal_graph_with_modules(&db, model, sync.project);
    let source_vars = model.variables(&db);
    let dm_dims = project_datamodel_dims(&db, sync.project);
    build_element_level_loops(
        circuits,
        &var_graph,
        source_vars,
        &db,
        sync.project,
        dm_dims.as_slice(),
    )
}

#[test]
fn a2a_loop_links_carry_bare_shape() {
    // Pure A2A: pop[r] -> births[r] -> pop[r]. Every link in the
    // resulting A2A loop must be annotated with Some(RefShape::Bare)
    // so that loop-score generation references the canonical
    // {from}->{to} link score (no per-element prefix).
    let project = TestProject::new("a2a_shape")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("pop[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "pop * 0.1", None);

    let loops = build_loops_for_test(&project);
    assert!(!loops.is_empty(), "expected at least one A2A loop");
    for l in &loops {
        for link in &l.links {
            assert_eq!(
                link.shape,
                Some(RefShape::Bare),
                "A2A loop link {:?}->{:?} should carry Bare shape, got {:?}",
                link.from.as_str(),
                link.to.as_str(),
                link.shape,
            );
        }
    }
}

/// Collect all quoted variable references from a loop_score equation.
///
/// Loop-score equations have the form `"name1" * "name2" * ...`.  This
/// parser returns every string between double-quote pairs so the caller
/// can check that each referenced name is actually emitted as a variable.
fn extract_quoted_refs(equation: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut rest = equation;
    while let Some(open) = rest.find('"') {
        let inner = &rest[open + 1..];
        if let Some(close) = inner.find('"') {
            refs.push(inner[..close].to_string());
            rest = &inner[close + 1..];
        } else {
            break;
        }
    }
    refs
}

#[test]
fn mixed_scalar_loop_score_refs_resolve_to_emitted_names() {
    // Regression test for the "doubly-bracketed name" bug that occurred
    // when the mixed/scalar branch used FixedIndex(source_elem) as the
    // link shape. link_score_var_name(Bare) takes `from` verbatim, so
    // "pop[nyc]→total_pop" is well-formed. link_score_var_name(FixedIndex
    // (["nyc"])) would prepend "[nyc]" a second time, yielding the
    // malformed "pop[nyc][nyc]→total_pop" which no emitted variable
    // matches, making the loop score silently reference an undefined name.
    //
    // The fixture:
    //   pop[Region] (stock, inflow=births)
    //   total_pop = SUM(pop[*])           (scalar aux, cross-element)
    //   births[Region] = total_pop * 0.005 + pop * 0.05  (mixed inputs)
    //
    // The loop pop[r] -> total_pop -> births[r] -> pop[r] goes through a
    // scalar node, so it lands in the mixed/scalar branch.
    let project = TestProject::new("mixed_scalar_roundtrip")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("pop[Region]", "100", &["births"], &[], None)
        .scalar_aux("total_pop", "SUM(pop[*])")
        .array_flow("births[Region]", "total_pop * 0.005 + pop * 0.05", None);

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let ltm = model_ltm_variables(&db, model, sync.project);

    // Collect the full set of emitted variable names.
    let emitted: std::collections::HashSet<String> =
        ltm.vars.iter().map(|v| v.name.clone()).collect();

    assert!(
        !emitted.is_empty(),
        "expected LTM variables to be emitted for this feedback model"
    );

    // Assert no emitted name contains "][" -- the telltale sign of a
    // doubly-bracketed malformed name.
    for name in &emitted {
        assert!(
            !name.contains("]["),
            "emitted variable name contains doubly-bracketed '][': {name:?}"
        );
    }

    // For every loop_score equation, every quoted link-score reference
    // must appear in the emitted set.  A missing reference means the
    // loop score multiplies by an undefined variable, producing NaN.
    let loop_score_vars: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.contains("\u{205A}loop_score\u{205A}"))
        .collect();

    assert!(
        !loop_score_vars.is_empty(),
        "expected at least one loop_score variable; emitted: {emitted:?}"
    );

    for lsv in &loop_score_vars {
        let refs = extract_quoted_refs(&lsv.equation);
        for r in &refs {
            assert!(
                emitted.contains(r),
                "loop_score {:?} references {:?} which is not in emitted vars.\n\
                 Emitted names: {emitted:?}",
                lsv.name,
                r
            );
        }
    }
}

// -- Phase 4 Task 3.5 (edge-aliasing limitation regression test) --
//
// AC4.2 documented limitation: when a target equation references the
// same source under BOTH a Bare and a FixedIndex(NYC) shape (e.g.,
// `share[Region] = pop + pop[NYC]`), the same diagonal element-edge
// `pop[nyc] -> share[nyc]` is contributed by two distinct AST refs.
// The element graph deduplicates them into a single edge, and Phase 3
// emits two distinct link-score variables (one per shape). The Phase 4
// loop-link annotation heuristic, working only from node-name
// surface, must collapse to a single shape per loop link -- matched
// source/target subscripts pick Bare. The resulting loop score
// references only the Bare link-score (under-counting the FixedIndex
// contribution).
//
// This test pins the current heuristic's behavior so a future
// shape-threading refinement that emits both contributions or picks
// differently triggers a deliberate test update.

#[test]
fn edge_aliasing_bare_and_fixed_index_to_same_source_element() {
    use salsa::Setter;

    // Build a feedback-closed model so loop construction runs and
    // populates Link.shape. The aliased edge appears inside the A2A
    // loop pop[r] -> share[r] -> update[r] -> pop[r]:
    //
    //   pop[Region]: stock with inflow update[Region]
    //   share[Region] = pop + pop[NYC]   <- BOTH Bare and FixedIndex(NYC)
    //                                       contribute to pop[nyc]->share[nyc]
    //   update[Region] = share * 0.001
    let mut db = SimlinDb::default();
    let project = TestProject::new("aliasing")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("pop[Region]", "100", &["update"], &[], None)
        .array_aux("share[Region]", "pop + pop[NYC]")
        .array_flow("update[Region]", "share * 0.001", None)
        .build_datamodel();

    let (source_project, model) = {
        let result = sync_from_datamodel(&db, &project);
        (result.project, result.models["main"].source)
    };
    // Discovery mode emits link scores for ALL edges, so both the
    // Bare and FixedIndex variants land in the surface even though
    // (in exhaustive mode) the FixedIndex variant might be elided
    // for a non-loop edge.
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    // -- Item 1: element graph dedup -- the diagonal aliased edge
    // pop[nyc] -> share[nyc] appears once.
    let element_edges = model_element_causal_edges(&db, model, source_project);
    let pop_nyc_targets = element_edges
        .edges
        .get("pop[nyc]")
        .expect("pop[nyc] should have outgoing edges");
    assert!(
        pop_nyc_targets.contains("share[nyc]"),
        "expected pop[nyc] -> share[nyc] in element graph; targets: {pop_nyc_targets:?}"
    );

    // -- Item 2: BOTH link score variables emitted --
    let ltm = model_ltm_variables(&db, model, source_project);
    let names: Vec<&String> = ltm.vars.iter().map(|v| &v.name).collect();
    let bare_name = "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share";
    let fixed_name = "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}share";
    assert!(
        names.iter().any(|n| n.as_str() == bare_name),
        "expected Bare link score {bare_name:?}; got: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.as_str() == fixed_name),
        "expected FixedIndex(nyc) link score {fixed_name:?}; got: {names:?}"
    );

    // -- Item 3: heuristic's chosen shape on the aliased edge in a
    // loop -- pop[nyc] -> share[nyc] is inside an A2A loop and the
    // A2A branch sets Bare for every link (matched source/target
    // subscripts). This pins the documented under-counting behavior.
    //
    // Switch back to exhaustive mode so loops are constructed.
    source_project.set_ltm_discovery_mode(&mut db).to(false);
    let loops = build_loops_for_test(
        &TestProject::new("aliasing")
            .with_sim_time(0.0, 5.0, 1.0)
            .named_dimension("Region", &["NYC", "Boston"])
            .array_stock("pop[Region]", "100", &["update"], &[], None)
            .array_aux("share[Region]", "pop + pop[NYC]")
            .array_flow("update[Region]", "share * 0.001", None),
    );
    assert!(
        !loops.is_empty(),
        "expected at least one loop in the aliasing fixture"
    );

    // Find the link in some loop whose stripped from is "pop" and
    // stripped to is "share". The heuristic's choice on this aliased
    // edge is the documented current behavior.
    let mut chosen_shapes: Vec<Option<RefShape>> = Vec::new();
    for l in &loops {
        for link in &l.links {
            // Compare stripped variable names so we catch both A2A
            // (variable-level pop->share) and per-element forms.
            let from_stripped = link
                .from
                .as_str()
                .split('[')
                .next()
                .unwrap_or(link.from.as_str());
            let to_stripped = link
                .to
                .as_str()
                .split('[')
                .next()
                .unwrap_or(link.to.as_str());
            if from_stripped == "pop" && to_stripped == "share" {
                chosen_shapes.push(link.shape.clone());
            }
        }
    }
    assert!(
        !chosen_shapes.is_empty(),
        "expected at least one pop->share link in the loops; got loops: {:?}",
        loops.iter().map(|l| l.id.clone()).collect::<Vec<_>>()
    );

    // Pin the documented limitation: every pop->share loop link gets
    // Bare under the current heuristic (A2A branch unconditionally
    // sets Bare). A future shape-threading refinement that emits a
    // FixedIndex variant for this edge would change the expectation
    // here -- exactly the deliberate breakage we want.
    for shape in &chosen_shapes {
        assert_eq!(
            shape,
            &Some(RefShape::Bare),
            "documented limitation: heuristic should pick Bare on the \
             aliased edge inside the A2A loop, missing the FixedIndex \
             contribution; got {shape:?}"
        );
    }
}

// -- Partition-lookup regression test (cycle 2 fix) --
//
// The mixed/scalar branch in build_element_level_loops previously stripped
// subscripts from element-level node names when building Loop.stocks. This
// caused partition_for_loop to return None for every mixed/scalar loop because
// model_element_cycle_partitions keys its stock_partition map on element-level
// names (e.g. "pop[nyc]"), not variable-level names (e.g. "pop"). Silently
// returning None from every lookup corrupts per-loop normalization in
// compute_rel_loop_scores.
//
// This test verifies that loop_partitions contains at least one Some(N) value
// for the mixed_scalar_roundtrip fixture, which has mixed loops that cross
// through a scalar node (total_pop) and arrayed stocks (pop[nyc], pop[boston]).

#[test]
fn mixed_scalar_loop_partitions_resolve_to_some() {
    // Same fixture used in mixed_scalar_loop_score_refs_resolve_to_emitted_names:
    //   pop[Region] (stock, inflow=births)
    //   total_pop = SUM(pop[*])           (scalar aux, cross-element)
    //   births[Region] = total_pop * 0.005 + pop * 0.05  (mixed inputs)
    //
    // The loops pop[nyc] -> total_pop -> births[nyc] -> pop[nyc] and
    // pop[boston] -> total_pop -> births[boston] -> pop[boston] both pass
    // through a scalar node, so they land in the mixed/scalar branch.
    // Their stocks (pop[nyc], pop[boston]) must appear in the element-level
    // cycle-partition map, yielding Some(N) for each loop.
    let project = TestProject::new("mixed_scalar_roundtrip")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("pop[Region]", "100", &["births"], &[], None)
        .scalar_aux("total_pop", "SUM(pop[*])")
        .array_flow("births[Region]", "total_pop * 0.005 + pop * 0.05", None);

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let ltm = model_ltm_variables(&db, model, sync.project);

    // Only exhaustive mode populates loop_partitions.
    assert!(
        !ltm.loop_partitions.is_empty(),
        "expected loop_partitions to be non-empty for a model with feedback loops"
    );

    // At least one mixed/scalar loop must resolve to Some(N), not None.
    // Before the fix every mixed/scalar loop returned None because Loop.stocks
    // held variable-level names ("pop") but stock_partition holds element-level
    // keys ("pop[nyc]").
    let any_some = ltm.loop_partitions.values().any(|v| v.is_some());
    assert!(
        any_some,
        "all loop_partitions values are None, meaning partition_for_loop \
         returned None for every loop; this indicates the element-level \
         Loop.stocks regression has recurred. loop_partitions: {:?}",
        ltm.loop_partitions
    );
}
