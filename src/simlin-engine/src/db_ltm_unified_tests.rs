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

/// A pure A2A stock-flow loop over a large dimension produces many
/// element-level circuits but `build_element_level_loops` collapses
/// them into a single A2A `Loop`.  The total-circuit backstop keys on
/// the post-collapse distinct-signature count, so these models stay on
/// the exhaustive path regardless of `|dimension|` -- only ~9 KB of
/// Loop materialization, well under the 10k-loop cliff the backstop
/// guards against.  A regression where we keyed on raw element-level
/// `circuits_result.len()` would force these models into discovery
/// mode and lose their per-loop scores.
#[test]
fn test_pure_a2a_over_large_dimension_stays_exhaustive() {
    let dim_size = crate::ltm::MAX_LTM_TOTAL_CIRCUITS + 1;
    let elements: Vec<String> = (0..dim_size).map(|i| format!("R{i}")).collect();
    let elem_refs: Vec<&str> = elements.iter().map(String::as_str).collect();

    let project = crate::test_common::TestProject::new("arrayed_a2a_stays_exhaustive")
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
        "pure-A2A arrayed model with |Region|={} collapses to one \
         variable-level loop; must stay exhaustive and emit loop_score. \
         MAX_LTM_TOTAL_CIRCUITS = {}. Got vars: {:?}",
        dim_size,
        crate::ltm::MAX_LTM_TOTAL_CIRCUITS,
        ltm.vars
            .iter()
            .map(|v| &v.name)
            .take(20)
            .collect::<Vec<_>>()
    );
}

/// Build a project with N independent disjoint 3-node stock-flow
/// cycles.  Each cycle contributes one distinct variable-level loop
/// signature and one element-level circuit (no cross-element), so this
/// is the shape that actually exercises the total-circuit backstop:
/// N > MAX_LTM_TOTAL_CIRCUITS forces auto-flip because each cycle is a
/// separate variable-level signature that would materialize its own
/// Loop struct.  Contrasted with the pure-A2A test above which also has
/// N > threshold in element-count but collapses to one signature.
fn build_n_disjoint_cycles_project(project_name: &str, n: usize) -> crate::datamodel::Project {
    let mut builder = crate::test_common::TestProject::new(project_name);
    for k in 0..n {
        let aux_name = format!("aux_{k}");
        let flow_name = format!("flow_{k}");
        let stock_name = format!("stock_{k}");
        builder = builder.scalar_aux(&aux_name, &stock_name);
        builder = builder.flow(&flow_name, &aux_name, None);
        builder = builder.stock(&stock_name, "0", &[flow_name.as_str()], &[], None);
    }
    builder.build_datamodel()
}

/// The total-circuit backstop fires on models whose DISTINCT
/// variable-level loop count exceeds `MAX_LTM_TOTAL_CIRCUITS`, not on
/// raw element-level count.  N disjoint 3-node cycles have N distinct
/// signatures, so N > MAX_LTM_TOTAL_CIRCUITS must flip to discovery.
#[test]
fn test_auto_flip_on_distinct_signature_count_above_threshold() {
    // Each cycle = 1 distinct signature; 1 above threshold forces flip.
    let n = crate::ltm::MAX_LTM_TOTAL_CIRCUITS + 1;
    let project = build_n_disjoint_cycles_project("n_disjoint_cycles_flip", n);

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;

    let ltm = model_ltm_variables(&db, model, sync.project);

    let has_loop_score = ltm
        .vars
        .iter()
        .any(|v| v.name.contains("\u{205A}loop_score\u{205A}"));
    assert!(
        !has_loop_score,
        "total-circuit backstop must fire for {} > MAX_LTM_TOTAL_CIRCUITS \
         ({}) distinct variable-level signatures: expected discovery-shape \
         output (no loop_score vars), got vars: {:?}",
        n,
        crate::ltm::MAX_LTM_TOTAL_CIRCUITS,
        ltm.vars
            .iter()
            .map(|v| &v.name)
            .take(20)
            .collect::<Vec<_>>()
    );
}

/// The total-circuit backstop must emit its own Assembly Warning when it
/// fires so FFI / CLI callers see why exhaustive mode was skipped
/// (parallel to the largest-SCC gate's warning).
#[test]
fn test_auto_flip_on_total_circuits_emits_warning() {
    use crate::db::{CompilationDiagnostic, DiagnosticError, DiagnosticSeverity};

    let n = crate::ltm::MAX_LTM_TOTAL_CIRCUITS + 1;
    let project = build_n_disjoint_cycles_project("n_disjoint_cycles_warning", n);

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;

    let _ = model_ltm_variables(&db, model, sync.project);

    let diags = model_ltm_variables::accumulated::<CompilationDiagnostic>(&db, model, sync.project);

    let has_total_circuits_warning = diags.iter().any(|CompilationDiagnostic(d)| {
        d.severity == DiagnosticSeverity::Warning
            && matches!(
                &d.error,
                DiagnosticError::Assembly(msg)
                    if msg.contains("MAX_LTM_TOTAL_CIRCUITS")
                        && msg.contains("feedback loops")
            )
    });
    assert!(
        has_total_circuits_warning,
        "total-circuit backstop must emit an Assembly Warning; got: {:?}",
        diags.iter().map(|c| &c.0).collect::<Vec<_>>()
    );
}

/// Adversarial test for codex iter-7 P2 finding 1 (the "streaming
/// enumeration cap" concern).  A model with many disjoint small
/// cycles (each a separate SCC, each one Loop) must auto-flip to
/// discovery *during* Johnson's DFS rather than after materializing
/// every indexed path, otherwise a WASM caller can still OOM on
/// enumeration before the post-hoc distinct-loop gate fires.  Pure
/// A2A models that would otherwise stay exhaustive are covered by
/// the separate `test_pure_a2a_over_large_dimension_stays_exhaustive`
/// test; this one specifically exercises the
/// `MAX_LTM_ENUMERATION_CAP` bail.
///
/// We don't synthesize a >1M-cycle model in-test (too slow); instead
/// we pick N just above `MAX_LTM_TOTAL_CIRCUITS` (10_001) so the
/// streaming bail inside `model_element_loop_circuits` stays well
/// below its `MAX_LTM_ENUMERATION_CAP` (1_000_000) but the post-hoc
/// emit-count gate in `model_ltm_variables` still fires on the
/// distinct-Loop count.  A regression where
/// `model_element_loop_circuits` silently enumerated unbounded would
/// not be caught by this test, but one where the post-hoc gate
/// stopped firing on N disjoint cycles would.
#[test]
fn test_element_loop_circuits_streaming_cap_keeps_wasm_bounded() {
    use super::model_element_loop_circuits;

    let n = crate::ltm::MAX_LTM_TOTAL_CIRCUITS + 1;
    let project = build_n_disjoint_cycles_project("element_stream_cap", n);

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;

    let element = model_element_loop_circuits(&db, model, sync.project);
    // With n = 10_001, enumeration completes (well under the 1_000_000
    // streaming cap).  The circuits list has exactly n circuits, one
    // per disjoint cycle.
    assert!(
        !element.truncated,
        "{n} disjoint cycles is below MAX_LTM_ENUMERATION_CAP; enumeration must not truncate"
    );
    assert_eq!(element.circuits.len(), n);
}

/// Symmetric to `test_auto_flip_warning_surfaces_via_collect_model_diagnostics`
/// for the SCC gate: the total-circuit backstop's warning must also
/// reach `collect_model_diagnostics`, which is the API FFI callers
/// (libsimlin, simlin-mcp) use to surface compile-time issues to end
/// users.  A regression where `model_all_diagnostics` stopped driving
/// LTM synthesis would silence this path and leave users wondering why
/// `rel_loop_score` queries returned empty.
#[test]
fn test_total_circuits_warning_surfaces_via_collect_model_diagnostics() {
    use crate::db::{DiagnosticError, DiagnosticSeverity, collect_model_diagnostics};
    use salsa::Setter;

    let n = crate::ltm::MAX_LTM_TOTAL_CIRCUITS + 1;
    let project = build_n_disjoint_cycles_project("n_disjoint_cycles_surface", n);

    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let diags = collect_model_diagnostics(&db, source_model, source_project);

    let has_total_circuits_warning = diags.iter().any(|d| {
        d.severity == DiagnosticSeverity::Warning
            && matches!(
                &d.error,
                DiagnosticError::Assembly(msg)
                    if msg.contains("MAX_LTM_TOTAL_CIRCUITS")
                        && msg.contains("feedback loops")
            )
    });
    assert!(
        has_total_circuits_warning,
        "total-circuit backstop warning must reach collect_model_diagnostics; got: {:?}",
        diags
    );
}
