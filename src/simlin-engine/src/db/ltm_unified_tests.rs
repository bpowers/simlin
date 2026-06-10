// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::datamodel;
use crate::test_common::TestProject;
use crate::testutils::{feedback_loop_project, x_aux, x_flow, x_model, x_module, x_stock};

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
            !var.equation.source_text().is_empty(),
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

    assert_eq!(
        ltm.mode,
        crate::db::LtmMode::Discovery,
        "explicitly requesting discovery mode resolves to Discovery"
    );

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

/// Verify that scalar-to-arrayed edges produce one scalar link score per
/// target element, named `$⁚ltm⁚link_score⁚{from}→{to}[{elem}]` with
/// empty dimensions -- NOT a single Bare-A2A var with `dimensions =
/// [target_dims]`.
///
/// The Bare-A2A form was undiscoverable: `parse_link_offsets`'s
/// `expand_a2a_link_offsets` subscripts *both* sides over `target_dims`,
/// inventing a `growth_factor[nyc]` node that doesn't match the
/// unsubscripted `growth_factor` node coming from other edges -- so a
/// loop through `growth_factor` is unreachable in the search graph. The
/// per-target-element scalar form (mirroring the arrayed->scalar
/// `{from}[{elem}]→{to}` convention) parses via the `[`-in-`to`
/// single-passthrough branch to `(growth_factor, births[nyc])`.
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

    let names: std::collections::HashSet<&str> = ltm.vars.iter().map(|v| v.name.as_str()).collect();

    // One scalar link score per target element, with the element in the name.
    for elem in ["nyc", "boston", "la"] {
        let expected =
            format!("$\u{205A}ltm\u{205A}link_score\u{205A}growth_factor\u{2192}births[{elem}]");
        assert!(
            names.contains(expected.as_str()),
            "expected per-target-element scalar link score {expected:?}; emitted link scores: {:?}",
            ltm.vars
                .iter()
                .filter(|v| v.name.contains("link_score"))
                .map(|v| v.name.as_str())
                .collect::<Vec<_>>()
        );
        let lsv = ltm.vars.iter().find(|v| v.name == expected).unwrap();
        assert!(
            lsv.dimensions.is_empty(),
            "per-target-element link score {expected:?} must be scalar (empty dimensions), got {:?}",
            lsv.dimensions
        );
        // The equation references the target element on the `to` side (in the
        // statically-resolvable qualified `dimension·element` form -- the
        // variable NAME keeps the bare form), the scalar source
        // unsubscripted, and is a guard-form expression.
        let eq = lsv.equation.source_text();
        assert!(
            eq.contains(&format!("births[region·{elem}]")),
            "equation for {expected:?} should reference births[region·{elem}], got: {eq}"
        );
        assert!(
            eq.contains("growth_factor") && !eq.contains(&format!("growth_factor[{elem}]")),
            "equation for {expected:?} should reference growth_factor unsubscripted, got: {eq}"
        );
        assert!(
            eq.contains("if (TIME = INITIAL_TIME)"),
            "equation for {expected:?} should be a link-score guard form, got: {eq}"
        );
    }

    // The Bare-A2A var must NOT be emitted for a scalar->arrayed edge.
    let bare_a2a = "$\u{205A}ltm\u{205A}link_score\u{205A}growth_factor\u{2192}births";
    assert!(
        !names.contains(bare_a2a),
        "scalar->arrayed edge must not emit the Bare-A2A link score {bare_a2a:?}"
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

    assert_eq!(
        ltm.mode,
        crate::db::LtmMode::Discovery,
        "a 51-node SCC must auto-flip the resolved mode to Discovery"
    );

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

    assert_eq!(
        ltm.mode,
        crate::db::LtmMode::Exhaustive,
        "a 49-node SCC stays under the threshold and resolves to Exhaustive"
    );

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

/// Circuit-budget truncation (a dense SCC under the node-count gate, here
/// simulated with the test override) must flip `model_ltm_variables` to
/// discovery-mode shape -- link scores for causal edges, NO per-loop
/// `loop_score` vars -- and emit a Warning naming the budget. The trap this
/// guards: a truncated tiered result carries empty circuit lists, which the
/// pre-budget code would have read as "no loops / nothing to score" and
/// returned an empty exhaustive result, silently disagreeing with
/// `model_ltm_mode`.
#[test]
fn test_model_ltm_variables_circuit_budget_truncation_flips_to_discovery() {
    use crate::db::{CompilationDiagnostic, DiagnosticError, DiagnosticSeverity};

    let _guard = crate::ltm::LtmCircuitBudgetGuard::new(1);

    let project = crate::test_common::TestProject::new("budget_trip_vars")
        .stock("s", "100", &["in_f"], &["out_f"], None)
        .flow("in_f", "s * 0.1", None)
        .flow("out_f", "s * 0.05", None)
        .build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;

    let ltm = model_ltm_variables(&db, model, sync.project);

    assert_eq!(
        ltm.mode,
        crate::db::LtmMode::Discovery,
        "circuit-budget truncation must resolve the mode to Discovery"
    );
    assert!(
        ltm.vars.iter().any(|v| v.name.contains("link_score")),
        "flipped model must still emit discovery-shape link scores; got: {:?}",
        ltm.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
    assert!(
        !ltm.vars
            .iter()
            .any(|v| v.name.contains("\u{205A}loop_score\u{205A}")),
        "flipped model must NOT materialize loop_score vars from a truncated \
         (incomplete) circuit list"
    );

    let diags = model_ltm_variables::accumulated::<CompilationDiagnostic>(&db, model, sync.project);
    let has_budget_warning = diags.iter().any(|CompilationDiagnostic(d)| {
        d.severity == DiagnosticSeverity::Warning
            && matches!(
                &d.error,
                DiagnosticError::Assembly(msg)
                    if msg.contains("MAX_LTM_CIRCUITS") && msg.contains("discovery mode")
            )
    });
    assert!(
        has_budget_warning,
        "budget truncation should emit a Warning naming MAX_LTM_CIRCUITS; got: {:?}",
        diags.iter().map(|c| &c.0).collect::<Vec<_>>()
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
/// here because it is the exact entry point `libsimlin` and `simlin-mcp`
/// drive.
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

/// The `ltm_enabled` gate in `model_all_diagnostics` is what scopes LTM
/// diagnostic cost to callers who asked for LTM: when the flag is false,
/// `collect_model_diagnostics` must not run LTM synthesis, so a silently
/// auto-flipping model whose caller never requested LTM emits no LTM
/// diagnostics and pays no LTM synthesis cost.
///
/// This is the *mechanism* the GH #466 fix relies on, not a statement that
/// the auto-flip warning is permanently invisible. The FFI's
/// `simlin_project_get_errors` transiently re-enables `ltm_enabled` (under
/// the db lock, via `LtmEnabledGuard`) before collecting diagnostics *iff*
/// some simulation on the project was created with `enable_ltm = true` -- so
/// the warning IS reachable for those callers (covered end to end by
/// `simlin/tests/integration/errors.rs::test_get_errors_surfaces_ltm_auto_flip_warning_after_ltm_sim`).
/// What this test pins is the gate's "off by default" half: keep the original
/// intent -- never pay LTM cost when nobody asked for LTM -- machine-checkable
/// here, since that is exactly what makes the FFI's per-project scoping cheap.
#[test]
fn test_ltm_disabled_gate_suppresses_auto_flip_warning() {
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

// ---------------------------------------------------------------------------
// LTM synthetic-fragment compile-failure diagnostics
// ---------------------------------------------------------------------------

/// Build a model whose LTM augmentation emits a synthetic equation the
/// fragment compiler genuinely rejects, so the diagnostic pass has a real
/// failure to surface.
///
/// This used to be a `SMTH1`-in-loop model, but that hazard (the
/// composite-reference link score into a stdlib-macro module) was fixed in
/// GH #548 (`build_submodel_metadata` now registers the sub-model's LTM
/// composite var, so the cross-module reference resolves). The fixture is a
/// *variable-backed* partial reducer (`inflow[D1] = SUM(matrix[D1,*])` is
/// the whole RHS, so `inflow` itself is the agg) closed into a feedback loop
/// through a `D1` stock that broadcasts into the `D1 x D2` matrix.
/// Variable-backed aggs take the conservative cross-product in the element
/// graph, and the resulting loop-score equations reference link-score names
/// that don't exist for a partial reduce -- the bare A2A
/// `"...matrix→inflow"` and the subscripted-per-element
/// `"...matrix[a,x]→inflow"[b]` forms, where only the per-`(row, slot)`
/// scalar `matrix[a,x]→inflow[a]` vars are emitted -- so the loop-score
/// fragments fail to compile and `assemble_module` would silently stub them
/// to 0. The diagnostic pass must surface that. (This fixture originally
/// leaned on the GH #528 broadcast over-subscription of a *synthetic*
/// arrayed agg, which is now fixed; the variable-backed loop-score
/// name-resolution gap here is a distinct, still-open failure.)
///
/// Using a genuinely-unfixed failure (rather than a once-broken-now-fixed
/// one) keeps these diagnostic-infrastructure tests decoupled from any
/// single bug's lifetime -- the test-coupling concern of GH #547.
fn build_model_with_failing_ltm_fragment(name: &str) -> datamodel::Project {
    TestProject::new(name)
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .array_aux("matrix[D1,D2]", "stock[D1] * 0.1")
        .array_stock("stock[D1]", "10", &["inflow"], &[], None)
        .array_flow("inflow[D1]", "SUM(matrix[D1,*])", None)
        .build_datamodel()
}

/// Predicate: a diagnostic is an LTM synthetic-fragment compile-failure
/// `Warning` (as opposed to the auto-flip warning, which is also an
/// `Assembly` `Warning`).
fn is_ltm_fragment_failure(d: &crate::db::Diagnostic) -> bool {
    use crate::db::{DiagnosticError, DiagnosticSeverity};
    d.severity == DiagnosticSeverity::Warning
        && matches!(
            &d.error,
            DiagnosticError::Assembly(msg) if msg.contains("failed to compile")
        )
}

/// An LTM synthetic fragment that fails to compile must surface as a
/// `Warning` through `collect_model_diagnostics` -- the collector both
/// `libsimlin` and `simlin-mcp` hand to end users -- when `ltm_enabled`.
/// Without this the failure is silent: the variable keeps a layout slot,
/// reads a constant 0, and the model still simulates, so the degraded
/// loop/link score masquerades as a correct result.
#[test]
fn test_ltm_fragment_compile_failure_surfaces_warning() {
    use crate::db::collect_model_diagnostics;
    use salsa::Setter;

    let project = build_model_with_failing_ltm_fragment("frag_fail_surface");
    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let diags = collect_model_diagnostics(&db, source_model, source_project);

    let frag_failures: Vec<_> = diags
        .iter()
        .filter(|d| is_ltm_fragment_failure(d))
        .collect();
    assert!(
        !frag_failures.is_empty(),
        "an LTM synthetic fragment that fails to compile must surface a \
         Warning through collect_model_diagnostics; got: {diags:?}"
    );
    // The diagnostic must name the offending synthetic variable so a
    // caller can locate the degraded score.
    assert!(
        frag_failures.iter().all(|d| {
            d.variable
                .as_deref()
                .is_some_and(|v| v.contains("$\u{205A}ltm\u{205A}"))
        }),
        "fragment-failure warnings must name the LTM synthetic variable; \
         got: {frag_failures:?}"
    );
}

/// The compile-failure warning is accumulated by `model_ltm_fragment_diagnostics`
/// itself. Asserting on the tracked function directly isolates the
/// emission from the `model_all_diagnostics` wiring exercised by
/// `test_ltm_fragment_compile_failure_surfaces_warning`.
#[test]
fn test_model_ltm_fragment_diagnostics_emits_warning() {
    use crate::db::CompilationDiagnostic;
    use salsa::Setter;

    let project = build_model_with_failing_ltm_fragment("frag_fail_direct");
    let mut db = SimlinDb::default();
    let (source_project, model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    // Mirror the production reachability of this pass (`model_all_diagnostics`
    // only runs it when `ltm_enabled`); the broadcast-agg failure surfaces
    // regardless of the flag, but enabling it keeps the test faithful to how
    // the diagnostic is actually triggered.
    source_project.set_ltm_enabled(&mut db).to(true);

    model_ltm_fragment_diagnostics(&db, model, source_project);
    let diags = model_ltm_fragment_diagnostics::accumulated::<CompilationDiagnostic>(
        &db,
        model,
        source_project,
    );

    assert!(
        diags
            .iter()
            .any(|CompilationDiagnostic(d)| is_ltm_fragment_failure(d)),
        "model_ltm_fragment_diagnostics must accumulate a compile-failure \
         Warning for the broadcast arrayed-aggregate loop score; got: {:?}",
        diags.iter().map(|c| &c.0).collect::<Vec<_>>()
    );
}

/// Regression guard: a model whose LTM synthetic fragments all compile
/// cleanly (a plain scalar feedback loop) must emit ZERO
/// fragment-failure warnings. Surfacing failures must not become a
/// false-positive generator for healthy models.
#[test]
fn test_clean_ltm_model_emits_no_fragment_warning() {
    use crate::db::collect_model_diagnostics;
    use salsa::Setter;

    // A 5-node scalar cycle: every link score is scalar Bare and every
    // loop score is scalar -- the bread-and-butter LTM path, all of
    // which compiles.
    let project = build_chain_scc_project("clean_ltm_frag", 5);
    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let diags = collect_model_diagnostics(&db, source_model, source_project);

    let frag_failures: Vec<_> = diags
        .iter()
        .filter(|d| is_ltm_fragment_failure(d))
        .collect();
    assert!(
        frag_failures.is_empty(),
        "a model whose LTM fragments all compile must emit no \
         fragment-failure warnings; got: {frag_failures:?}"
    );
}

/// Counterpart to the surfacing test: when LTM is disabled,
/// `collect_model_diagnostics` must not run the LTM fragment-diagnostic
/// pass -- a model with a failing LTM fragment whose caller never asked
/// for LTM should not emit the warning. Mirrors
/// `test_ltm_disabled_gate_suppresses_auto_flip_warning`.
#[test]
fn test_ltm_disabled_does_not_surface_fragment_failure_warning() {
    use crate::db::collect_model_diagnostics;

    let project = build_model_with_failing_ltm_fragment("frag_fail_disabled");
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;

    assert!(
        !sync.project.ltm_enabled(&db),
        "baseline: ltm_enabled must default to false"
    );

    let diags = collect_model_diagnostics(&db, source_model, sync.project);

    let frag_failures: Vec<_> = diags
        .iter()
        .filter(|d| is_ltm_fragment_failure(d))
        .collect();
    assert!(
        frag_failures.is_empty(),
        "LTM-disabled project must not emit LTM fragment-failure \
         warnings; got: {frag_failures:?}"
    );
}

/// GH #311: the ceteris-paribus partial-equation parse-failure `Warning`
/// must name the offending variable AND the original equation text, and
/// explain the silent-magnitude-1 hazard the fix prevents. The message body
/// is a pure function so this contract is pinned without driving a salsa
/// accumulator -- the parse failure itself is effectively unreachable
/// through the production salsa LTM path (the equation text is always a
/// `print_eqn` re-print, and an empty equation is rejected as an
/// `EmptyEquation` *Error* upstream before LTM augmentation runs), so the
/// fix is defense-in-depth for the unanticipated case. The builder-level
/// loud-failure behavior is exercised directly in `ltm_augment`'s unit
/// tests; here we lock down the diagnostic wording the db-bearing callers
/// surface.
#[test]
fn test_ltm_partial_equation_warning_message_contract() {
    let var_name = "$\u{205A}ltm\u{205A}link_score\u{205A}source\u{2192}target";
    let eqn_text = "source * other";
    let msg = super::ltm::ltm_partial_equation_warning_message(var_name, eqn_text);

    assert!(
        msg.contains(var_name),
        "the warning must name the skipped link-score variable; got: {msg}"
    );
    assert!(
        msg.contains(eqn_text),
        "the warning must include the original equation text; got: {msg}"
    );
    assert!(
        msg.contains("ceteris-paribus"),
        "the warning must explain the ceteris-paribus requirement; got: {msg}"
    );
    assert!(
        msg.contains("magnitude of 1"),
        "the warning must explain the silent magnitude-1 hazard; got: {msg}"
    );
}

/// Test-only salsa-tracked query that drives the production accumulating
/// emitter [`emit_ltm_partial_equation_warning`] with a synthetic
/// `PartialEquationError`. Salsa accumulators can only be appended from
/// inside a tracked query, so this is the minimal harness that exercises the
/// real `.accumulate(db)` path the six production call sites share -- as
/// opposed to `test_ltm_partial_equation_warning_message_contract`, which
/// only covers the pure message body.
#[cfg(test)]
#[salsa::tracked]
fn ltm311_emit_probe(db: &dyn crate::db::Db, model: SourceModel, _project: SourceProject) {
    let err = crate::ltm_augment::PartialEquationError {
        equation_text: "source * other".to_string(),
    };
    super::ltm::emit_ltm_partial_equation_warning(
        db,
        model,
        "$\u{205A}ltm\u{205A}link_score\u{205A}source\u{2192}target",
        &err,
    );
}

/// GH #311 end-to-end through salsa: a `PartialEquationError` handed to the
/// db-bearing emitter must accumulate a `CompilationDiagnostic` with
/// `Warning` severity, the skipped variable's name in `variable`, and a
/// message carrying the variable name + the offending equation text. This
/// covers the actual salsa-accumulation glue the six production call sites
/// (db.rs, db/ltm/compile.rs, db/ltm/link_scores.rs) reuse; the production
/// route that *produces* the error is effectively unreachable (the equation
/// text is always a `print_eqn` re-print, and an empty equation is rejected
/// as an `EmptyEquation` Error upstream before LTM augmentation runs), which
/// is documented on `PartialEquationError`.
#[test]
fn test_ltm_partial_equation_warning_accumulates_via_salsa() {
    use crate::db::{CompilationDiagnostic, DiagnosticError, DiagnosticSeverity};

    let db = SimlinDb::default();
    let project = feedback_loop_project();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;

    ltm311_emit_probe(&db, model, sync.project);
    let diags = ltm311_emit_probe::accumulated::<CompilationDiagnostic>(&db, model, sync.project);

    let warning = diags
        .iter()
        .map(|CompilationDiagnostic(d)| d)
        .find(|d| {
            d.severity == DiagnosticSeverity::Warning
                && matches!(
                    &d.error,
                    DiagnosticError::Assembly(msg) if msg.contains("magnitude of 1")
                )
        })
        .unwrap_or_else(|| {
            panic!(
                "the partial-equation parse failure must accumulate a Warning via salsa; got: {:?}",
                diags.iter().map(|c| &c.0).collect::<Vec<_>>()
            )
        });

    let var_name = "$\u{205A}ltm\u{205A}link_score\u{205A}source\u{2192}target";
    assert_eq!(
        warning.variable.as_deref(),
        Some(var_name),
        "the Warning must name the skipped link-score variable in its `variable` field"
    );
    let DiagnosticError::Assembly(msg) = &warning.error else {
        unreachable!("matched Assembly above")
    };
    assert!(
        msg.contains(var_name) && msg.contains("source * other"),
        "the Warning message must carry the variable name + offending equation text; got: {msg}"
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

// -- Per-shape link scores --
//
// When a target equation references a source under multiple distinct
// RefShapes, model_ltm_variables emits a distinct LtmSyntheticVar per
// shape: a `Bare` ref keeps the canonical `{from}\u{2192}{to}` name, and a
// `FixedIndex` ref carries the per-element prefixed-from form
// (`{from}[{elem}]\u{2192}{to}`). `Wildcard` / `DynamicIndex` reducer
// references are *not* emitted as per-shape `\u{205A}wildcard` /
// `\u{205A}dynamic` variants (those were retired): a maximal inlined
// reducer is hoisted into a `$\u{205A}ltm\u{205A}agg\u{205A}{n}` aggregate
// node whose two link-score halves (`{from}[{d}]\u{2192}agg`,
// `agg\u{2192}{to}[{e}]`) carry the per-element edges instead. The
// not-hoisted conservative-slice and bare-dynamic-index cases still reach
// `emit_per_shape_link_scores` as a `Wildcard` / `DynamicIndex` shape, but
// they reuse the canonical Bare name (the access shape only drives which
// references the partial holds live, not the variable name). Discovery
// mode is used here so the link emission loop runs for every causal edge,
// not just edges in detected loops.

#[test]
fn per_shape_link_scores_for_share_with_sum() {
    use salsa::Setter;

    // share[R] = pop / SUM(pop[*]) references `pop` under both Bare (the
    // numerator) and Wildcard (inside SUM). Phase 5: the Bare ref still
    // produces the canonical `pop→share` link score, but the Wildcard ref
    // is routed through the synthetic agg `$⁚ltm⁚agg⁚0`, so it produces
    // `$⁚ltm⁚link_score⁚pop[d]→$⁚ltm⁚agg⁚0` (per source element) and
    // `$⁚ltm⁚link_score⁚$⁚ltm⁚agg⁚0→share[r]` (per target element) -- and
    // NOT a `pop→share⁚wildcard` var.
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
    let names: std::collections::HashSet<&str> = ltm.vars.iter().map(|v| v.name.as_str()).collect();

    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    // The Bare numerator's link score (unchanged).
    assert!(
        names.contains("$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share"),
        "expected Bare-shape link score pop→share; got: {names:?}"
    );
    // The synthetic agg itself.
    assert!(
        names.contains(agg),
        "expected synthetic agg {agg}; got: {names:?}"
    );
    // pop[d] → agg, one per source element.
    for d in &["nyc", "boston"] {
        let n = format!("$\u{205A}ltm\u{205A}link_score\u{205A}pop[{d}]\u{2192}{agg}");
        assert!(
            names.contains(n.as_str()),
            "expected per-source-element reducer link score {n:?}; got: {names:?}"
        );
    }
    // agg → share[r], one per target element (Phase 3 per-target-element form).
    for r in &["nyc", "boston"] {
        let n = format!("$\u{205A}ltm\u{205A}link_score\u{205A}{agg}\u{2192}share[{r}]");
        assert!(
            names.contains(n.as_str()),
            "expected agg→share[{r}] link score {n:?}; got: {names:?}"
        );
    }
    // No `⁚wildcard` / `⁚dynamic` var anymore.
    assert!(
        names
            .iter()
            .all(|n| !n.ends_with("\u{205A}wildcard") && !n.ends_with("\u{205A}dynamic")),
        "no ⁚wildcard / ⁚dynamic link scores must be emitted; got: {names:?}"
    );
}

/// AC5.1 (ltm-503-cross-element-agg): the `⁚wildcard` / `⁚dynamic`
/// per-shape link-score path is retired. Reducer references are routed
/// through synthetic `$⁚ltm⁚agg⁚{n}` aggregate nodes instead, so no
/// `model_ltm_variables` output ever carries those shape suffixes.
///
/// This is a positive guard over a handful of reducer-bearing fixtures
/// (a `share`-with-feedback model, a whole-RHS `SUM` model, and a `MEAN`
/// reducer model): for each we fetch `model_ltm_variables` and assert
/// that no synthetic variable name contains `⁚wildcard` or `⁚dynamic`.
#[test]
fn no_wildcard_or_dynamic_link_scores_for_reducer_models() {
    fn assert_no_shape_suffix_vars(label: &str, project: &datamodel::Project) {
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, project);
        let model = sync.models["main"].source;
        let ltm = model_ltm_variables(&db, model, sync.project);
        assert!(
            !ltm.vars.is_empty(),
            "{label}: expected LTM variables for the reducer-bearing fixture"
        );
        for v in &ltm.vars {
            assert!(
                !v.name.contains("\u{205A}wildcard") && !v.name.contains("\u{205A}dynamic"),
                "{label}: no ⁚wildcard / ⁚dynamic link scores must be emitted; \
                 offending var: {:?}; all vars: {:?}",
                v.name,
                ltm.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
            );
        }
    }

    // `share[r] = pop[r] / SUM(pop[*])` with feedback through `update`:
    // the maximal `SUM(pop[*])` subexpression is hoisted into a synthetic
    // agg, and the cross-element loop is scored on the element-level path.
    let share_with_feedback = TestProject::new("share_feedback")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("pop[Region]", "100", &["update"], &[], None)
        .array_aux("share[Region]", "pop / SUM(pop[*])")
        .array_flow("update[Region]", "share * 0.001", None)
        .build_datamodel();
    assert_no_shape_suffix_vars("share_with_feedback", &share_with_feedback);

    // `total_pop = SUM(pop[*])` is a *whole-RHS* reducer -- a
    // variable-backed agg, no synthetic minted -- but it must still not
    // produce a `⁚wildcard`-suffixed link score for any consumer edge.
    let total_pop = TestProject::new("total_pop_sum")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("pop[Region]", "100", &["growth"], &[], None)
        .scalar_aux("total_pop", "SUM(pop[*])")
        .array_flow("growth[Region]", "pop * 0.01 + total_pop * 0.0001", None)
        .build_datamodel();
    assert_no_shape_suffix_vars("total_pop", &total_pop);

    // A `MEAN` reducer feeding back through a scalar adjustment.
    let mean_model = TestProject::new("mean_reducer")
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_stock("pop[Region]", "100", &["adjust"], &[], None)
        .scalar_aux("avg_pop", "MEAN(pop[*])")
        .array_flow("adjust[Region]", "(avg_pop - pop) * 0.01", None)
        .build_datamodel();
    assert_no_shape_suffix_vars("mean_model", &mean_model);
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

/// Regression test: the FixedIndex link score's source-delta normalizer
/// must reference the FixedIndex element (e.g., `pop[nyc]`), not the
/// variable-level `from` (`pop`).
///
/// For `rel_pop[r] = pop / pop[NYC]`:
///   - Bare link score `pop→rel_pop` partial leaves bare `pop` live and
///     wraps `pop[NYC]` in PREVIOUS. Source delta should be `Δpop` (per
///     element under A2A) -- correct today.
///   - FixedIndex link score `pop[nyc]→rel_pop` partial leaves
///     `pop[nyc]` live and wraps bare `pop`. Source delta should be
///     `Δpop[nyc]`, but the buggy version used `Δpop`, so under A2A the
///     denominator became `Δpop[r]` at each target element -- wrong
///     source. This distorts magnitude and can flip the loop-score sign
///     when `pop[nyc]` and `pop[r]` move in opposite directions.
#[test]
fn fixed_index_link_score_denominator_uses_fixed_element() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = TestProject::new("rel_pop_denom")
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

    let fixed_name = "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}rel_pop";
    let fixed = ltm
        .vars
        .iter()
        .find(|v| v.name == fixed_name)
        .expect("expected FixedIndex(nyc) link score");
    let fixed_eq = fixed.equation.source_text();

    // The denominator that drives the SIGN of the link score must
    // reference the NYC element of pop (the FixedIndex element kept live
    // in the partial), not the bare variable-level `pop`. The element is
    // spelled in the statically-resolvable qualified `dimension·element`
    // form so the PREVIOUS-wrapped occurrence compiles to a direct
    // LoadPrev (no helper aux).
    assert!(
        fixed_eq.contains("(pop[region·nyc] - PREVIOUS(pop[region·nyc]))"),
        "FixedIndex link score denominator must reference pop[region·nyc]; got: {fixed_eq}",
    );
    // It must NOT contain the unsuffixed `(pop - PREVIOUS(pop))` form,
    // which under A2A becomes `Δpop[r]` and normalizes by the wrong
    // source.
    assert!(
        !fixed_eq.contains("(pop - PREVIOUS(pop))"),
        "FixedIndex link score must not normalize by the unsuffixed Δpop; got: {fixed_eq}",
    );

    // The Bare variant must still use the unsuffixed source delta --
    // its partial keeps the bare `pop` live, so `Δpop` (per element
    // under A2A) is the correct normalizer.
    let bare_name = "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}rel_pop";
    let bare = ltm
        .vars
        .iter()
        .find(|v| v.name == bare_name)
        .expect("expected Bare link score");
    let bare_eq = bare.equation.source_text();
    assert!(
        bare_eq.contains("(pop - PREVIOUS(pop))"),
        "Bare link score must keep its unsuffixed Δpop denominator; got: {bare_eq}",
    );
}

// -- Loop-link naming in build_element_level_loops --
//
// AC4.1 / AC4.2: build_element_level_loops must produce link names that
// match the link-score variables actually emitted, so the loop-score
// equation references resolve.
//
// Pure A2A loops use variable-level names on both ends. Mixed/scalar
// loops normalize as follows:
//  - Cross-dimensional edges (subscripted from, bare to): element-level
//    from is preserved so the loop score references the per-element link
//    score emitted by try_cross_dimensional_link_scores.
//  - All other edges (A2A inside a mixed loop, scalar-to-arrayed, etc.):
//    subscripts are stripped so the loop score references the variable-level
//    A2A or scalar link score emitted by emit_per_shape_link_scores.
//
// An earlier version threaded a per-link `RefShape` through `Link` and
// drove the per-element name from `link_score_var_name(FixedIndex)`. That
// produced doubly-bracketed names like "population[nyc][nyc]→total_pop"
// because the helper prepends "[nyc]" to a from name that was already
// element-level. Encoding the per-element distinction in `link.from`
// directly removes the structural mismatch.

/// Build the element-level loops for a TestProject by replicating the
/// same orchestration `model_ltm_variables` does internally.
/// `build_element_level_loops` is `pub(crate)` so tests can inspect the
/// link-name normalization rules directly.
///
/// Drives the legacy `model_element_loop_circuits` (now `#[deprecated]`
/// for new LTM callers) on purpose -- these tests pin the behavior of
/// the slow-path consumer `build_element_level_loops` independently of
/// the tiered enumerator's dedup logic.
#[allow(deprecated)]
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
        MAX_CROSS_AGG_LOOPS,
    )
    .0
}

#[test]
fn a2a_loop_links_use_variable_level_names() {
    // Pure A2A: pop[r] -> births[r] -> pop[r]. The A2A branch of
    // build_element_level_loops must produce links with variable-level
    // (no-subscript) names so the loop-score generation references the
    // canonical {from}->{to} link score that emit_per_shape_link_scores
    // produces.
    let project = TestProject::new("a2a_shape")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("pop[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "pop * 0.1", None);

    let loops = build_loops_for_test(&project);
    assert!(!loops.is_empty(), "expected at least one A2A loop");
    for l in &loops {
        for link in &l.links {
            assert!(
                !link.from.as_str().contains('['),
                "A2A loop link from should be variable-level, got {:?}",
                link.from.as_str(),
            );
            assert!(
                !link.to.as_str().contains('['),
                "A2A loop link to should be variable-level, got {:?}",
                link.to.as_str(),
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
        let refs = extract_quoted_refs(&lsv.equation.source_text());
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

/// ltm-503-cross-element-agg.AC2.5: a model with no arrayed variables has
/// its loop-score equations unchanged -- they reference unsubscripted
/// scalar link scores exactly as before, with no `[elem]` subscript.
#[test]
fn scalar_model_loop_score_has_no_element_subscript() {
    let db = SimlinDb::default();
    let project = feedback_loop_project();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let ltm = model_ltm_variables(&db, model, sync.project);

    let loop_score_vars: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.contains("\u{205A}loop_score\u{205A}"))
        .collect();
    assert_eq!(
        loop_score_vars.len(),
        1,
        "the scalar feedback model has exactly one loop (population -> births -> population); \
         got: {:?}",
        loop_score_vars.iter().map(|v| &v.name).collect::<Vec<_>>(),
    );
    let eq = loop_score_vars[0].equation.source_text();
    // No element subscript anywhere: every reference is a bare quoted name.
    assert!(
        !eq.contains('['),
        "scalar-model loop-score equation must not contain any `[elem]` subscript; got: {eq}",
    );
    // It is the product of the two scalar link scores.
    let refs = extract_quoted_refs(&eq);
    let expected: std::collections::HashSet<&str> = [
        "$\u{205A}ltm\u{205A}link_score\u{205A}population\u{2192}births",
        "$\u{205A}ltm\u{205A}link_score\u{205A}births\u{2192}population",
    ]
    .into_iter()
    .collect();
    let got: std::collections::HashSet<&str> = refs.iter().map(|s| s.as_str()).collect();
    assert_eq!(
        got, expected,
        "loop-score equation should reference exactly the two bare scalar link scores; got: {eq}",
    );
    assert!(
        eq.contains(" * "),
        "loop-score equation should be a product; got: {eq}",
    );
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

// `model_element_loop_circuits` is `#[deprecated]`; this test pins
// pre-tiered loop-link annotation behavior on purpose.
#[allow(deprecated)]
#[test]
fn edge_aliasing_bare_and_fixed_index_to_same_source_element() {
    use salsa::Setter;

    // Build a feedback-closed model so loop construction runs. The
    // aliased edge appears inside the A2A loop
    // pop[r] -> share[r] -> update[r] -> pop[r]:
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

    // -- Item 3: link-name form for the aliased edge inside a loop --
    // pop[nyc] -> share[nyc] is inside an A2A loop and the A2A branch
    // strips subscripts on both ends, so the loop links use
    // variable-level "pop" rather than per-element "pop[nyc]". The
    // loop-score equation therefore multiplies against the canonical
    // Bare-named link score and misses the FixedIndex(NYC) contribution
    // that emit_per_shape_link_scores also produces. This pins the
    // documented under-counting behavior.
    //
    // Switch back to exhaustive mode (same db and project, no rebuild)
    // so build_element_level_loops runs.
    source_project.set_ltm_discovery_mode(&mut db).to(false);
    let circuits = model_element_loop_circuits(&db, model, source_project);
    let loops = if circuits.is_empty() {
        vec![]
    } else {
        let var_graph = causal_graph_with_modules(&db, model, source_project);
        let source_vars = model.variables(&db);
        let dm_dims = project_datamodel_dims(&db, source_project);
        build_element_level_loops(
            circuits,
            &var_graph,
            source_vars,
            &db,
            source_project,
            dm_dims.as_slice(),
            MAX_CROSS_AGG_LOOPS,
        )
        .0
    };
    assert!(
        !loops.is_empty(),
        "expected at least one loop in the aliasing fixture"
    );

    // Find the link in some loop whose stripped from is "pop" and
    // stripped to is "share". The link's `from` form on this aliased
    // edge encodes the documented current behavior.
    let mut chosen_from_names: Vec<String> = Vec::new();
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
                chosen_from_names.push(link.from.as_str().to_string());
            }
        }
    }
    assert!(
        !chosen_from_names.is_empty(),
        "expected at least one pop->share link in the loops; got loops: {:?}",
        loops.iter().map(|l| l.id.clone()).collect::<Vec<_>>()
    );

    // Pin the documented limitation: every pop->share loop link uses
    // a variable-level `from` ("pop"), not the per-element FixedIndex
    // form ("pop[nyc]"). The A2A branch of build_element_level_loops
    // strips subscripts uniformly, so the loop-score equation
    // multiplies against the canonical Bare-named link score and
    // misses the FixedIndex(NYC) contribution that emit_per_shape_link_scores
    // also produces. A future shape-threading refinement that emits a
    // FixedIndex variant inside the loop would surface here as a
    // bracketed `from` -- exactly the deliberate breakage we want.
    for from_name in &chosen_from_names {
        assert!(
            !from_name.contains('['),
            "documented limitation: A2A loop link should use \
             variable-level pop, missing the FixedIndex contribution; \
             got {from_name:?}"
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
fn cross_element_loop_partitions_resolve_to_some() {
    // The cross-element wildcard-reducer fixture (used elsewhere by
    // `cross_element_loop_through_sum_reducer` in db/element_graph_tests):
    //
    //   population[Region] (stock, inflow=births)
    //   births[Region] = SUM(population[*]) * 0.01
    //
    // The element graph contains a 4-node cross-element circuit
    // population[nyc] -> births[boston] -> population[boston] -> births[nyc]
    // -> population[nyc]. `build_element_level_loops`'s cross-element
    // branch collapses this to a scalar loop with `dimensions: vec![]`.
    //
    // The `Loop` docstring's stocks-granularity invariant says any loop
    // with `dimensions.is_empty()` MUST carry element-level stock names
    // because `partition_for_loop` keys
    // `model_element_cycle_partitions::stock_partition` element-level.
    // The cross-element branch was using variable-level stocks, so its
    // partition lookup returned None and the loop bucketed into the
    // None group in `compute_rel_loop_scores` -- silently corrupting
    // per-loop normalization (the cross-element loop should be in the
    // partition containing the population[*] stocks).
    let project = TestProject::new("cross_element_partition")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("population[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "SUM(population[*]) * 0.01", None);

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let ltm = model_ltm_variables(&db, model, sync.project);

    assert!(
        !ltm.loop_partitions.is_empty(),
        "expected loop_partitions for the cross-element fixture"
    );

    // Identify loop_score variables by id and inspect their dimensions
    // to find loops with empty `dimensions` (i.e., cross-element /
    // mixed / scalar). Per the `Loop` docstring's invariant, those
    // MUST resolve to a Some partition. A2A loops (non-empty
    // dimensions) legitimately return None because they don't use the
    // element-level partition lookup.
    let mut scalar_loop_ids: Vec<String> = Vec::new();
    for v in &ltm.vars {
        if let Some(id) = v
            .name
            .strip_prefix("$\u{205A}ltm\u{205A}loop_score\u{205A}")
            && v.dimensions.is_empty()
        {
            scalar_loop_ids.push(id.to_string());
        }
    }
    assert!(
        !scalar_loop_ids.is_empty(),
        "expected at least one cross-element/scalar loop in the fixture; \
         vars: {:?}",
        ltm.vars
            .iter()
            .filter(|v| v.name.contains("loop_score"))
            .map(|v| (&v.name, &v.dimensions))
            .collect::<Vec<_>>()
    );

    for id in &scalar_loop_ids {
        let partition = ltm
            .loop_partitions
            .get(id)
            .unwrap_or_else(|| panic!("loop {id:?} missing from loop_partitions"));
        // A scalar / cross-element loop has exactly one slot.
        assert_eq!(
            partition.len(),
            1,
            "scalar / cross-element loop {id:?} should have one partition slot, got {partition:?}"
        );
        assert!(
            partition[0].is_some(),
            "scalar / cross-element loop {id:?} resolved to None partition; \
             cross-element branch must produce element-level stocks per the \
             `Loop` docstring's invariant. loop_partitions: {:?}",
            ltm.loop_partitions,
        );
    }
}

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
    // keys ("pop[nyc]").  (`partition_for_loop` now returns a per-slot vector;
    // mixed/scalar loops have one slot.)
    let any_some = ltm
        .loop_partitions
        .values()
        .any(|slots| slots.iter().any(|p| p.is_some()));
    assert!(
        any_some,
        "all loop_partitions values are None, meaning partition_for_loop \
         returned None for every loop; this indicates the element-level \
         Loop.stocks regression has recurred. loop_partitions: {:?}",
        ltm.loop_partitions
    );
}

#[test]
fn a2a_loop_partitions_have_one_entry_per_element() {
    // A pure-A2A stock-flow loop over a 3-element dimension whose elements
    // are *not* cross-coupled (each `pop[r]` only depends on `pop[r]`):
    // `loop_partitions[a2a_loop_id]` has one entry per element, and because
    // `model_element_cycle_partitions` puts the three element-level stocks
    // in three distinct SCCs, the three entries are three distinct partition
    // indices.  Pre-#487 the A2A loop carried variable-level stocks
    // (`"pop"`) so `partition_for_loop` returned a single `None`; now it
    // returns `[Some(p0), Some(p1), Some(p2)]` in the runtime's row-major
    // slot order -- so the rel-loop-score normalizer can keep the three
    // per-element subsystems in separate `(partition, slot)` buckets.
    let project = TestProject::new("a2a_partition")
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_stock("pop[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "pop * 0.1", None);

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let ltm = model_ltm_variables(&db, model, sync.project);

    // Find the A2A loop_score variable (it has non-empty `dimensions`).
    let mut a2a_loop_ids: Vec<String> = Vec::new();
    for v in &ltm.vars {
        if let Some(id) = v
            .name
            .strip_prefix("$\u{205A}ltm\u{205A}loop_score\u{205A}")
            && !v.dimensions.is_empty()
        {
            a2a_loop_ids.push(id.to_string());
        }
    }
    assert_eq!(
        a2a_loop_ids.len(),
        1,
        "expected exactly one A2A loop; loop_score vars: {:?}",
        ltm.vars
            .iter()
            .filter(|v| v.name.contains("loop_score"))
            .map(|v| (&v.name, &v.dimensions))
            .collect::<Vec<_>>()
    );
    let a2a_id = &a2a_loop_ids[0];
    let parts = ltm
        .loop_partitions
        .get(a2a_id)
        .unwrap_or_else(|| panic!("A2A loop {a2a_id:?} missing from loop_partitions"));
    assert_eq!(
        parts.len(),
        3,
        "A2A loop over a 3-element dimension should have 3 partition slots, got {parts:?}"
    );
    assert!(
        parts.iter().all(|p| p.is_some()),
        "every slot of the A2A loop should resolve to a partition, got {parts:?}"
    );
    let distinct: std::collections::HashSet<usize> = parts.iter().filter_map(|p| *p).collect();
    assert_eq!(
        distinct.len(),
        3,
        "the 3 element-wise-uncoupled slots should be in 3 distinct partitions, got {parts:?}"
    );
}

/// GH #468: `loop_partitions` must iterate in *emission* order -- the
/// content-derived order `assign_loop_ids` produces and
/// `model_ltm_variables` inserts the per-loop partition vectors in --
/// not lexicographic loop-id order and not `HashMap` hash-randomized
/// order.
///
/// Emission order is what the post-sim `compute_rel_loop_scores*`
/// denominator summation walks; pinning it restores bit-for-bit parity
/// with the pre-#461 compile-time emitter (IEEE-754 addition is
/// non-associative, so `r1+r2+...+r10` differs at the ULP from the
/// lex-sorted `r1+r10+r2+...`).  The fixture puts 12 loops in one
/// partition so the two orders genuinely differ (`r1, r10, r11, r12,
/// r2, ...` lex vs `r1, r2, ..., r12` emission).
#[test]
fn loop_partitions_iterate_in_emission_order() {
    // One stock with 12 parallel feedback paths through 12 distinct
    // auxes.  Every loop `s -> a{i} -> netflow -> s` shares the single
    // SCC partition, and all are reinforcing, so they take sequential
    // `r1..r12` ids in `assign_loop_ids`' content-sorted order.
    let mut p = TestProject::new("emission_order_loops");
    let mut inflow_terms: Vec<String> = Vec::new();
    for i in 1..=12 {
        p = p.scalar_aux(&format!("a{i}"), "s * 0.001");
        inflow_terms.push(format!("a{i}"));
    }
    let p = p.stock("s", "100", &["netflow"], &[], None).flow(
        "netflow",
        &inflow_terms.join(" + "),
        None,
    );

    let datamodel = p.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let ltm = model_ltm_variables(&db, model, sync.project);

    // The independent oracle: the emission order is `assign_loop_ids`'
    // content-sorted order, which `model_detected_loops` reproduces
    // (it runs the same enumeration + id assignment, with no pins here).
    // NOTE: this oracle shares `assign_loop_ids` with the code under test, so
    // it pins that `loop_partitions` agrees with `model_detected_loops` but
    // CANNOT catch an `assign_loop_ids`-internal ordering bug (both would shift
    // together); the dedicated `assign_loop_ids_is_order_independent_*` tests
    // cover that.
    let detected = model_detected_loops(&db, model, sync.project);
    let emission_order: Vec<String> = detected.loops.iter().map(|l| l.id.clone()).collect();
    assert_eq!(
        emission_order.len(),
        12,
        "fixture must enumerate 12 loops; got {emission_order:?}"
    );

    let iter_order: Vec<String> = ltm.loop_partitions.keys().cloned().collect();
    assert_eq!(
        iter_order, emission_order,
        "loop_partitions must iterate in emission order (the order \
         `assign_loop_ids` / `model_detected_loops` produce), so the \
         post-sim rel-score denominator sums in emission order"
    );

    // Guard against a future refactor that "fixes" determinism by
    // lex-sorting: with 12 same-prefix ids the emission order is *not*
    // the lexicographic order, so this distinguishes the two.
    let mut lex_order = iter_order.clone();
    lex_order.sort();
    assert_ne!(
        iter_order, lex_order,
        "fixture is supposed to make emission order differ from lex order; \
         if these are equal the test no longer guards the bit-parity property"
    );
}

/// Regression test: every link-score reference inside a loop_score
/// equation must resolve to a synthetic variable that was actually
/// emitted. For `share[r] = SUM(pop[*])` the only reference of `pop` in
/// `share` is inside the maximal inlined reducer, so it is hoisted into
/// `$⁚ltm⁚agg⁚{n}` and the cross-element loop traverses
/// `pop[d] → agg → share[r] → update[r] → pop[r]`. The loop_score
/// equation must reference the agg-hop link scores (`pop[d]→agg`,
/// `agg→share[r]`) that were emitted -- if a stale resolver invented a
/// `pop→share` name that nothing produced, the fragment compiler would
/// quietly fall back to a stub dep and the loop would silently lose that
/// link's contribution.
#[test]
fn loop_score_picks_emitted_shape_when_only_wildcard_exists() {
    // share[r] depends on pop only via SUM(pop[*]) -- the reducer is
    // hoisted into a synthetic agg. update[r] feeds back into pop[r] via
    // the structural flow path. The cross-element loop
    // pop[r] -> agg -> share[r] -> update[r] -> pop[r] exists at the
    // element graph level.
    let project = TestProject::new("wildcard_only_loop")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("pop[Region]", "100", &["update"], &[], None)
        .array_aux("share[Region]", "SUM(pop[*])")
        .array_flow("update[Region]", "share * 0.001", None);

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let ltm = model_ltm_variables(&db, model, sync.project);

    let emitted: std::collections::HashSet<String> =
        ltm.vars.iter().map(|v| v.name.clone()).collect();

    assert!(
        !emitted.is_empty(),
        "expected LTM variables for the inlined-reducer feedback fixture"
    );

    let loop_score_vars: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.contains("\u{205A}loop_score\u{205A}"))
        .collect();

    assert!(
        !loop_score_vars.is_empty(),
        "expected at least one loop_score variable; emitted: {emitted:?}"
    );

    // Every link-score reference inside a loop_score equation must
    // resolve to a variable that was actually emitted.
    for lsv in &loop_score_vars {
        let refs = extract_quoted_refs(&lsv.equation.source_text());
        for r in &refs {
            assert!(
                emitted.contains(r),
                "loop_score {:?} references {:?} which is not in emitted vars.\n\
                 Expected the loop to route through the synthetic agg's two \
                 link-score halves.\nEmitted names matching pop / share / agg:\n  {}\n",
                lsv.name,
                r,
                emitted
                    .iter()
                    .filter(|n| n.contains("pop") || n.contains("share") || n.contains("agg"))
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("\n  "),
            );
        }
    }
}

#[test]
fn cross_dim_link_score_equations_match_between_exhaustive_and_discovery() {
    // Regression test for the silent correctness bug where exhaustive-mode
    // loop iteration passed element-level `link.from` ("pop[nyc]") to
    // `try_cross_dimensional_link_scores`, which looks up by variable name
    // ("pop") in `source_vars`. The lookup failed, the cross-dim helper
    // returned None, and the code fell through to the generic per-shape
    // emitter -- which has no AST anchor for "pop[nyc]" in total_pop's
    // equation, so it wrapped the bare `pop` in `SUM(pop[*])` in PREVIOUS,
    // making the numerator `sum(PREVIOUS(pop[*])) - PREVIOUS(total_pop)`,
    // which is identically zero. The emitted equation evaluated to 0 at
    // every timestep, silently zeroing the loop contribution from
    // cross-dimensional arrayed-to-scalar reducer edges.
    //
    // Discovery mode worked correctly because `edges_result.edges` is
    // variable-level, so `from = "pop"` and the cross-dim helper succeeds.
    //
    // Both modes must produce the same per-element link score formulas
    // for cross-dimensional edges.
    let project = TestProject::new("crossdim_match")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("pop[Region]", "100", &["births"], &[], None)
        .scalar_aux("total_pop", "SUM(pop[*])")
        .array_flow("births[Region]", "total_pop * 0.005 + pop * 0.05", None);

    let datamodel = project.build_datamodel();

    let db_ex = SimlinDb::default();
    let sync_ex = sync_from_datamodel(&db_ex, &datamodel);
    let model_ex = sync_ex.models["main"].source;
    let ltm_ex = model_ltm_variables(&db_ex, model_ex, sync_ex.project);

    use salsa::Setter;
    let mut db_disc = SimlinDb::default();
    let model_disc;
    let project_disc;
    {
        let sync_disc = sync_from_datamodel(&db_disc, &datamodel);
        model_disc = sync_disc.models["main"].source;
        project_disc = sync_disc.project;
    }
    project_disc.set_ltm_discovery_mode(&mut db_disc).to(true);
    let ltm_disc = model_ltm_variables(&db_disc, model_disc, project_disc);

    let by_name = |vars: &[LtmSyntheticVar]| -> std::collections::HashMap<String, String> {
        vars.iter()
            .map(|v| (v.name.clone(), v.equation.source_text()))
            .collect()
    };
    let ex_eqs = by_name(&ltm_ex.vars);
    let disc_eqs = by_name(&ltm_disc.vars);

    // The two cross-dimensional link scores that the bug zeroed out:
    for elem in &["nyc", "boston"] {
        let name = format!("$\u{205A}ltm\u{205A}link_score\u{205A}pop[{elem}]\u{2192}total_pop");
        let ex_eq = ex_eqs
            .get(&name)
            .unwrap_or_else(|| panic!("exhaustive mode missing cross-dim link score {name}"));
        let disc_eq = disc_eqs
            .get(&name)
            .unwrap_or_else(|| panic!("discovery mode missing cross-dim link score {name}"));
        assert_eq!(
            ex_eq, disc_eq,
            "exhaustive and discovery cross-dim link score equations differ for {name}\n\
             exhaustive:  {ex_eq}\n\
             discovery:   {disc_eq}",
        );
        // Defensive: the buggy form contained `sum(PREVIOUS(pop[*]))`
        // which evaluates to PREVIOUS(total_pop), making the SAFEDIV
        // numerator identically zero.
        assert!(
            !ex_eq.contains("sum(PREVIOUS(pop[*]))"),
            "exhaustive equation still contains the zero-numerator form: {ex_eq}",
        );
    }
}

/// ltm-503-cross-element-agg.AC4.6 (the machinery): a partial reduce
/// `agg[D1] = SUM(matrix[D1,*])` collapses only the D2 axis, leaving an
/// arrayed result over D1. The reducer link-score machinery must emit one
/// *scalar* link score per `(d1, d2)` pair, named
/// `$⁚ltm⁚link_score⁚matrix[d1,d2]→agg[d1]` (the source subscript carries
/// both axes; the target subscript only the surviving axis), each with
/// `dimensions = vec![]`. It must NOT emit a single A2A `matrix→agg` over
/// `D1` (that would broadcast over D1 in the discovery parser, producing
/// wrong edges) or a per-`(d1,d2)` var carrying `dimensions = ["D1"]`.
#[test]
fn partial_reduce_emits_per_source_element_scalar_link_scores() {
    let project = TestProject::new("partial_reduce_machinery")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .array_stock("matrix[D1,D2]", "1", &["growth"], &[], None)
        .array_flow("growth[D1,D2]", "matrix * 0.05", None)
        .array_aux("agg[D1]", "SUM(matrix[D1,*])");

    let datamodel = project.build_datamodel();

    use salsa::Setter;
    let mut db = SimlinDb::default();
    let model;
    let proj;
    {
        let sync = sync_from_datamodel(&db, &datamodel);
        model = sync.models["main"].source;
        proj = sync.project;
    }
    // Discovery mode visits every causal edge, so the matrix -> agg edge
    // is exercised without needing it to participate in a loop.
    proj.set_ltm_discovery_mode(&mut db).to(true);
    let ltm = model_ltm_variables(&db, model, proj);

    let by_name: std::collections::HashMap<String, &LtmSyntheticVar> =
        ltm.vars.iter().map(|v| (v.name.clone(), v)).collect();

    for (d1, d2) in [("a", "x"), ("a", "y"), ("b", "x"), ("b", "y")] {
        let name =
            format!("$\u{205A}ltm\u{205A}link_score\u{205A}matrix[{d1},{d2}]\u{2192}agg[{d1}]");
        let lsv = by_name.get(&name).unwrap_or_else(|| {
            panic!(
                "expected per-(d1,d2) partial-reduce link score {name}; emitted: {:?}",
                by_name.keys().collect::<Vec<_>>()
            )
        });
        assert!(
            lsv.dimensions.is_empty(),
            "partial-reduce link score {name} must be scalar (dimensions = []), got {:?}",
            lsv.dimensions
        );
        // The equation must reference the row element on the target side
        // and the full source tuple on the source side, both in the
        // statically-resolvable qualified `dimension·element` form (the
        // variable NAME keeps the bare form).
        let eq = lsv.equation.source_text();
        assert!(
            eq.contains(&format!("agg[d1·{d1}]")),
            "link score {name} equation should reference agg[d1·{d1}]: {eq}"
        );
        assert!(
            eq.contains(&format!("matrix[d1·{d1},d2·{d2}]")),
            "link score {name} equation should reference matrix[d1·{d1},d2·{d2}]: {eq}"
        );
    }

    // Must NOT emit a Bare A2A `matrix→agg` (no element subscript on
    // either side) -- with or without dimensions.
    assert!(
        !by_name.contains_key("$\u{205A}ltm\u{205A}link_score\u{205A}matrix\u{2192}agg"),
        "must not emit a Bare A2A matrix->agg link score; emitted: {:?}",
        by_name.keys().collect::<Vec<_>>()
    );
    // And no per-(d1,d2) variant should carry D1 dimensions.
    for v in &ltm.vars {
        if v.name
            .starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}matrix[")
        {
            assert!(
                v.dimensions.is_empty(),
                "partial-reduce link score {} must not carry dimensions, got {:?}",
                v.name,
                v.dimensions
            );
        }
    }
}

/// ltm-503-cross-element-agg.AC3.2 (exhaustive loop-score side): the
/// loop `population[nyc] -> total_pop -> migration[nyc] ->
/// population[nyc]` (a scalar reducer factored out of the per-element
/// migration flow) has its loop-score equation built from exactly three
/// per-element link-score references along its element-level path:
///   - `"$⁚ltm⁚link_score⁚population[nyc]→total_pop"` -- the arrayed->scalar
///     reducer link score, per source element (from `try_cross_dimensional_link_scores`),
///   - `"$⁚ltm⁚link_score⁚total_pop→migration[nyc]"` -- the scalar->arrayed
///     link score, per target element (from `try_scalar_to_arrayed_link_scores`),
///   - `"$⁚ltm⁚link_score⁚migration→population"[nyc]` -- the structural
///     flow->stock A2A link score, subscripted-after-quote at the visited
///     element.
///
/// In particular it must NOT reference a Bare-A2A `total_pop→migration`
/// name (no longer emitted) nor a same-element diagonal of it.
#[test]
fn scalar_reducer_loop_score_uses_per_element_link_scores() {
    let project = TestProject::new("scalar_reducer_loop")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock(
            "population[Region]",
            "100",
            &["births", "migration"],
            &[],
            None,
        )
        .array_aux("birth_rate[Region]", "0.05")
        .array_flow("births[Region]", "population * birth_rate", None)
        .scalar_aux("total_pop", "SUM(population[*])")
        .array_flow(
            "migration[Region]",
            "total_pop * 0.01 - population * 0.01",
            None,
        );

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let ltm = model_ltm_variables(&db, model, sync.project);

    let factors = |eq: &str| -> std::collections::HashSet<String> {
        eq.split(" * ").map(|s| s.trim().to_string()).collect()
    };
    let expected: std::collections::HashSet<String> = [
        "\"$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}total_pop\"".to_string(),
        "\"$\u{205A}ltm\u{205A}link_score\u{205A}total_pop\u{2192}migration[nyc]\"".to_string(),
        "\"$\u{205A}ltm\u{205A}link_score\u{205A}migration\u{2192}population\"[nyc]".to_string(),
    ]
    .into_iter()
    .collect();

    let loop_score_eqs: Vec<String> = ltm
        .vars
        .iter()
        .filter(|v| v.name.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}"))
        .map(|v| v.equation.source_text())
        .collect();
    assert!(
        loop_score_eqs.iter().any(|eq| factors(eq) == expected),
        "no loop_score equation has the scalar-reducer loop's per-element factor set {expected:?}; \
         loop_score equations present: {loop_score_eqs:?}"
    );

    // The Bare-A2A name for the scalar->arrayed edge is gone; no loop
    // score may reference it.
    for eq in &loop_score_eqs {
        assert!(
            !eq.contains("\"$\u{205A}ltm\u{205A}link_score\u{205A}total_pop\u{2192}migration\""),
            "loop_score equation references the retired Bare-A2A name `total_pop→migration`: {eq}"
        );
    }
}

// -- Phase 5 (aggregate nodes: $⁚ltm⁚agg⁚{n}) --
//
// A maximal inlined reducer subexpression that participates in feedback is
// hoisted into a synthetic auxiliary `$⁚ltm⁚agg⁚{n}` (computed during
// simulation, so `PREVIOUS(agg)` is available). A variable whose entire
// dt-equation is exactly one reducer call is its own aggregate node -- no
// synthetic is minted.

/// AC4.1 / AC4.3: `share[r] = pop[r] / SUM(pop[*])` with `share` feeding
/// back into `pop` mints a synthetic agg `$⁚ltm⁚agg⁚0` with equation text
/// `sum(pop[*])` and `dimensions = vec![]` (a scalar full reduce).
#[test]
fn agg_aux_emitted_for_hoisted_reducer() {
    let project = TestProject::new("agg_share")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("pop[Region]", "100", &["update"], &[], None)
        .array_aux("share[Region]", "pop / SUM(pop[*])")
        .array_flow("update[Region]", "share * 0.001", None);

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let ltm = model_ltm_variables(&db, model, sync.project);

    let agg_name = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let agg = ltm
        .vars
        .iter()
        .find(|v| v.name == agg_name)
        .unwrap_or_else(|| {
            panic!(
                "expected synthetic agg {agg_name:?}; got: {:?}",
                ltm.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
            )
        });
    assert!(
        agg.dimensions.is_empty(),
        "agg should be scalar: {:?}",
        agg.dimensions
    );
    assert!(
        matches!(&agg.equation, crate::datamodel::Equation::Scalar(t) if t == "sum(pop[*])"),
        "agg equation should be the reducer subexpr text; got: {:?}",
        agg.equation
    );
}

/// AC4.3: `total_population = SUM(population[*])` is a whole-RHS scalar
/// reducer -- it IS the aggregate node, so no `$⁚ltm⁚agg⁚{n}` is minted.
#[test]
fn no_agg_aux_for_whole_rhs_reducer() {
    let project = TestProject::new("whole_rhs_agg")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("population[Region]", "100", &["migration"], &[], None)
        .scalar_aux("total_population", "SUM(population[*])")
        .array_flow(
            "migration[Region]",
            "total_population * 0.001 - population * 0.001",
            None,
        );

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let ltm = model_ltm_variables(&db, model, sync.project);

    assert!(
        !ltm.vars
            .iter()
            .any(|v| v.name.contains("\u{205A}agg\u{205A}")),
        "whole-RHS reducer must not mint a synthetic agg; got: {:?}",
        ltm.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
}

/// GH #737: a pure-scalar feedback cycle that re-enters through a *scalar
/// feeder* of a hoisted reducer (`grow = 1 + SUM(pop[*] * scale)`, with
/// `scale` fed back from the scalar stock `total`) must be built routed
/// through the synthetic agg node -- links `scale → $⁚ltm⁚agg⁚0` and
/// `$⁚ltm⁚agg⁚0 → grow`, never a direct `scale → grow` link.
///
/// Pre-fix, `classify_cycle` saw only Bare shapes on the cycle's edges (the
/// scalar feeder's reference inside the reducer is Bare) and classified it
/// `PureScalar`; `build_loops_from_tiered`'s fast path then materialized the
/// loop straight from the variable-level circuit, linking `scale → grow`
/// directly -- an edge with no usable link-score variable. The cycle now
/// classifies `CrossElementOrMixed` (it traverses a `ThroughAgg`-routed
/// edge), descends to the element-level slow path, and the built `Loop`
/// traverses the agg.
///
/// The loop's polarity must also classify concretely (not degrade to
/// Undetermined): `recover_agg_hop_polarities` patches the agg hops (SUM is
/// monotone, `grow = 1 + agg` is a positive consumer) and the rest of the
/// cycle's signs are statically known, so the loop is reinforcing -- pinned
/// here via the `r`-prefixed loop_score id `model_ltm_variables` emits.
#[test]
fn scalar_feeder_cycle_routes_through_agg_node() {
    let project = TestProject::new("scalar_feeder_routing")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("pop[Region]", "100", &["pgrow"], &[], None)
        .array_flow("pgrow[Region]", "pop * 0.05", None)
        .aux("scale", "0.001 * total + 0.01", None)
        .stock("total", "100", &["grow"], &[], None)
        .flow("grow", "1 + SUM(pop[*] * scale)", None);

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;

    let tiered = model_loop_circuits_tiered(&db, model, sync.project);
    let var_graph = causal_graph_with_modules(&db, model, sync.project);
    let source_vars = model.variables(&db);
    let dm_dims = project_datamodel_dims(&db, sync.project);
    let (loops, _truncated) = build_loops_from_tiered(
        tiered,
        &var_graph,
        source_vars,
        &db,
        sync.project,
        dm_dims.as_slice(),
        MAX_CROSS_AGG_LOOPS,
    );

    let agg_name = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let scalar_loop = loops
        .iter()
        .find(|l| l.links.iter().any(|link| link.from.as_str() == "scale"))
        .unwrap_or_else(|| {
            panic!(
                "expected a loop containing scale; got: {:?}",
                loops
                    .iter()
                    .map(|l| l.links.iter().map(|k| k.from.as_str()).collect::<Vec<_>>())
                    .collect::<Vec<_>>()
            )
        });
    let edges: Vec<(&str, &str)> = scalar_loop
        .links
        .iter()
        .map(|l| (l.from.as_str(), l.to.as_str()))
        .collect();
    assert!(
        edges.contains(&("scale", agg_name)) && edges.contains(&(agg_name, "grow")),
        "the scalar feeder loop must traverse scale → {agg_name} → grow; links: {edges:?}"
    );
    assert!(
        !edges.contains(&("scale", "grow")),
        "the scalar feeder loop must NOT carry a direct scale → grow link; links: {edges:?}"
    );

    // Polarity end-to-end: the emitted loop score for the agg-routed loop is
    // r-prefixed (reinforcing), not the Undetermined `u{n}` fallback.
    let ltm = model_ltm_variables(&db, model, sync.project);
    let loop_score_prefix = "$\u{205A}ltm\u{205A}loop_score\u{205A}";
    let agg_loop_score = ltm
        .vars
        .iter()
        .find(|v| {
            v.name.starts_with(loop_score_prefix) && v.equation.source_text().contains(agg_name)
        })
        .unwrap_or_else(|| {
            panic!(
                "expected an agg-routed loop score; loop vars: {:?}",
                ltm.vars
                    .iter()
                    .filter(|v| v.name.starts_with(loop_score_prefix))
                    .map(|v| (&v.name, v.equation.source_text()))
                    .collect::<Vec<_>>()
            )
        });
    assert!(
        agg_loop_score.name[loop_score_prefix.len()..].starts_with('r'),
        "the agg-routed loop's polarity must classify concretely (reinforcing); got id {:?}",
        &agg_loop_score.name[loop_score_prefix.len()..]
    );
}

/// The agg aux must be emitted *before* the link-score variables in the
/// returned `vars` list (the LTM flow fragments are not topologically
/// sorted, and the `agg → target` link score reads the agg's current-step
/// value, so the agg fragment must run first in the same timestep).
#[test]
fn agg_aux_sorts_before_link_scores() {
    let project = TestProject::new("agg_sort")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("pop[Region]", "100", &["update"], &[], None)
        .array_aux("share[Region]", "pop / SUM(pop[*])")
        .array_flow("update[Region]", "share * 0.001", None);

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let ltm = model_ltm_variables(&db, model, sync.project);

    let agg_pos = ltm
        .vars
        .iter()
        .position(|v| v.name.contains("\u{205A}agg\u{205A}"))
        .expect("expected a synthetic agg variable");
    let first_link_score_pos = ltm
        .vars
        .iter()
        .position(|v| v.name.contains("\u{205A}link_score\u{205A}"));
    if let Some(ls) = first_link_score_pos {
        assert!(
            agg_pos < ls,
            "agg variable (at {agg_pos}) must sort before the first link score (at {ls}); \
             order: {:?}",
            ltm.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
        );
    }
}

/// AC4.2: a cross-element feedback loop through a hoisted reducer visits the
/// aggregate node twice -- it is NOT an elementary circuit, so Johnson can't
/// emit it directly. `build_loops_from_tiered` recovers it (combining the
/// per-element "petals" of the agg node), and its `$⁚ltm⁚loop_score⁚{id}`
/// equation is the product of the per-element link scores along the
/// un-trimmed path, including the `pop[d]→agg` and `agg→share[e]` halves with
/// `d ≠ e` (the cross-element coupling through the aggregate).
#[test]
fn cross_element_loop_through_agg_is_recovered() {
    let project = TestProject::new("cross_through_agg")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("pop[Region]", "100", &["update"], &[], None)
        .array_aux("share[Region]", "pop / SUM(pop[*])")
        .array_flow("update[Region]", "share * 0.001", None);

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let ltm = model_ltm_variables(&db, model, sync.project);

    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let loop_score_eqs: Vec<String> = ltm
        .vars
        .iter()
        .filter(|v| v.name.contains("\u{205A}loop_score\u{205A}"))
        .map(|v| v.equation.source_text())
        .collect();
    assert!(
        !loop_score_eqs.is_empty(),
        "expected loop_score variables; emitted: {:?}",
        ltm.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );

    // The cross-element-through-agg loop's loop-score equation must reference,
    // along the un-trimmed path, NYC's pop into the agg AND the agg into
    // Boston's share (the cross-element hop), AND the return: Boston's pop
    // into the agg AND the agg into NYC's share.
    let want_factors = [
        format!("\"$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}{agg}\""),
        format!("\"$\u{205A}ltm\u{205A}link_score\u{205A}{agg}\u{2192}share[boston]\""),
        format!("\"$\u{205A}ltm\u{205A}link_score\u{205A}pop[boston]\u{2192}{agg}\""),
        format!("\"$\u{205A}ltm\u{205A}link_score\u{205A}{agg}\u{2192}share[nyc]\""),
    ];
    let has_cross_through_agg = loop_score_eqs
        .iter()
        .any(|eq| want_factors.iter().all(|f| eq.contains(f.as_str())));
    assert!(
        has_cross_through_agg,
        "no loop_score equation traverses the cross-element-through-agg path \
         (NYC pop→agg→Boston share→...→Boston pop→agg→NYC share). \
         Want all of {want_factors:?}.\nloop_score equations: {loop_score_eqs:?}"
    );

    // And the agg-routed link-score halves it references must actually be
    // emitted (so the fragment compiler doesn't stub them to zero).
    let emitted: std::collections::HashSet<String> =
        ltm.vars.iter().map(|v| v.name.clone()).collect();
    for f in &want_factors {
        let bare = f.trim_matches('"');
        assert!(
            emitted.contains(bare),
            "loop_score equation references {bare:?} but it was not emitted; \
             emitted: {emitted:?}"
        );
    }

    // The user-facing reported loops (model_detected_loops, variable-level)
    // never include the synthetic agg node -- the aggregate is "trimmed" from
    // the displayed loop. (The element-level loops that carry the agg node
    // exist only internally, to build the loop-score equations.)
    let detected = crate::db::model_detected_loops(&db, model, sync.project);
    for l in &detected.loops {
        assert!(
            l.variables
                .iter()
                .all(|v| !v.contains("\u{205A}agg\u{205A}")),
            "model_detected_loops should not surface synthetic agg nodes; got: {:?}",
            l.variables
        );
    }

    // GH #516: the cross-element-through-agg loop must NOT classify as
    // Undetermined. Its agg hops are derivable -- `pop[d] → agg` is Positive
    // (SUM is monotone) and `agg → share[e]` is Negative (`share = pop / agg`,
    // the agg is the denominator) -- so the loop's id carries a definite
    // `r`/`b` prefix, not `u`. Find the loop_score var whose equation is the
    // un-trimmed cross-through-agg product and check its id prefix.
    let cross_agg_loop_score = ltm
        .vars
        .iter()
        .find(|v| {
            v.name.contains("\u{205A}loop_score\u{205A}")
                && want_factors
                    .iter()
                    .all(|f| v.equation.source_text().contains(f.as_str()))
        })
        .expect("expected a loop_score var for the cross-element-through-agg loop");
    let loop_id = cross_agg_loop_score
        .name
        .rsplit('\u{205A}')
        .next()
        .expect("loop_score var name has a trailing loop id");
    assert!(
        loop_id.starts_with('r') || loop_id.starts_with('b'),
        "cross-element-through-agg loop should have a determined polarity \
         (r/b), not Undetermined (u); loop_score var = {:?}",
        cross_agg_loop_score.name
    );
}

/// AC4.3 (#514): a *sliced* reducer subexpression (`SUM(pop[NYC,*])`) hoisted
/// into a synthetic agg gets per-element `source[d] → agg` link scores for
/// *only the rows it reads* -- `$⁚ltm⁚link_score⁚pop[nyc,adult]→agg` and
/// `$⁚ltm⁚link_score⁚pop[nyc,child]→agg` -- and *no* `pop[boston,*]→agg` link
/// scores. A cross-element feedback loop visiting NYC (`pop[nyc,age] → agg →
/// drive[nyc,age] → flow[nyc,age] → pop[nyc,age]`, the two NYC slots sharing
/// the agg) is enumerated and combined by `recover_cross_agg_loops`, its
/// loop-score equation references the per-slice link scores along the
/// un-trimmed path, and the reported loops never surface the synthetic agg.
#[test]
fn sliced_agg_link_scores_cover_only_the_read_rows() {
    let project = TestProject::new("sliced_agg")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .named_dimension("Age", &["Adult", "Child"])
        .array_stock("pop[Region,Age]", "100", &["flow"], &[], None)
        // An A2A aux with `SUM(pop[NYC,*])` as a *sub-expression* -> the
        // maximal `SUM(pop[NYC,*])` is hoisted into a synthetic agg, which
        // broadcasts to every `drive` element (so each `pop` slot's loop
        // through the agg has its own, disjoint, `drive`/`flow` nodes -- the
        // condition `recover_cross_agg_loops` needs to combine them).
        .array_aux("drive[Region,Age]", "SUM(pop[NYC,*]) * 0.0001")
        // `flow` is the same-element diagonal of `drive`, closing the loop.
        // Only the NYC slots actually feed the agg, so only they are in a
        // loop through it.
        .array_flow("flow[Region,Age]", "drive", None);

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let ltm = model_ltm_variables(&db, model, sync.project);
    let names: std::collections::HashSet<&str> = ltm.vars.iter().map(|v| v.name.as_str()).collect();

    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    // The agg itself: a scalar (no Iterated axis -- `SUM(pop[NYC,*])` is a
    // full reduce over the `*` axis with `NYC` pinned, not keyed by an A2A
    // dim) merely broadcast to the arrayed `drive`.
    let agg_var = ltm
        .vars
        .iter()
        .find(|v| v.name == agg)
        .unwrap_or_else(|| panic!("expected synthetic agg {agg}; got: {names:?}"));
    assert!(agg_var.dimensions.is_empty());
    assert!(
        matches!(&agg_var.equation, crate::datamodel::Equation::Scalar(t) if t == "sum(pop[nyc, *])"),
        "agg equation should be the sliced reducer text; got: {:?}",
        agg_var.equation
    );

    // `pop[nyc,*] → agg` link scores -- one per row the slice reads.
    for age in &["adult", "child"] {
        let n = format!("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc,{age}]\u{2192}{agg}");
        assert!(
            names.contains(n.as_str()),
            "expected per-read-row reducer link score {n:?}; got: {names:?}"
        );
    }
    // No `pop[boston,*] → agg` link scores -- Boston's rows are not read by
    // the `pop[NYC,*]` slice.
    for age in &["adult", "child"] {
        let n = format!("$\u{205A}ltm\u{205A}link_score\u{205A}pop[boston,{age}]\u{2192}{agg}");
        assert!(
            !names.contains(n.as_str()),
            "must NOT emit a link score for the unread row {n:?}; got: {names:?}"
        );
    }
    // `agg → drive[e]` link scores -- one per target element (arrayed `to`).
    for elem in &["nyc,adult", "nyc,child"] {
        let n = format!("$\u{205A}ltm\u{205A}link_score\u{205A}{agg}\u{2192}drive[{elem}]");
        assert!(
            names.contains(n.as_str()),
            "expected agg→drive[{elem}] link score; got: {names:?}"
        );
    }

    // A loop-score equation traverses the NYC cross-element path through the
    // agg: NYC-Adult into the agg, agg into drive[nyc,child], ... and
    // NYC-Child into the agg, agg into drive[nyc,adult]. Pin that the
    // per-read-row halves appear along the un-trimmed path (`recover_cross_agg_loops`
    // stitched the two NYC petals together).
    let loop_score_eqs: Vec<String> = ltm
        .vars
        .iter()
        .filter(|v| v.name.contains("\u{205A}loop_score\u{205A}"))
        .map(|v| v.equation.source_text())
        .collect();
    let want = [
        format!("\"$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc,adult]\u{2192}{agg}\""),
        format!("\"$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc,child]\u{2192}{agg}\""),
        format!("\"$\u{205A}ltm\u{205A}link_score\u{205A}{agg}\u{2192}drive[nyc,adult]\""),
        format!("\"$\u{205A}ltm\u{205A}link_score\u{205A}{agg}\u{2192}drive[nyc,child]\""),
    ];
    assert!(
        loop_score_eqs
            .iter()
            .any(|eq| want.iter().all(|f| eq.contains(f.as_str()))),
        "no loop_score equation traverses the NYC-through-sliced-agg path; \
         want all of {want:?}.\nloop_score equations: {loop_score_eqs:?}"
    );

    // The reported (variable-level) loops never surface the synthetic agg.
    let detected = crate::db::model_detected_loops(&db, model, sync.project);
    for l in &detected.loops {
        assert!(
            l.variables
                .iter()
                .all(|v| !v.contains("\u{205A}agg\u{205A}")),
            "model_detected_loops should not surface synthetic agg nodes; got: {:?}",
            l.variables
        );
    }
}

// ── Phase 5 (#515): budgeted cross-element-through-aggregate loop recovery ──

/// Build the canonical "reducer in a feedback loop over `Region`" fixture:
/// `pop[Region]` stock fed by `update[Region] = share[Region] * 0.001`, with
/// `share[Region] = pop[Region] / SUM(pop[*])`. The maximal reducer
/// `SUM(pop[*])` hoists into a synthetic scalar agg `$⁚ltm⁚agg⁚0` that every
/// `share` element reads, so for each region `r` there is one disjoint
/// "petal" `$⁚ltm⁚agg⁚0 → share[r] → update[r] → pop[r] → $⁚ltm⁚agg⁚0`. The
/// element graph also has the same-element diagonal `pop[r] → share[r]` (the
/// `pop[r]` numerator), so `pop` is read both directly and through the agg.
fn share_reducer_loop_fixture(n: usize) -> datamodel::Project {
    let elements: Vec<String> = (0..n).map(|i| format!("r{i}")).collect();
    let element_refs: Vec<&str> = elements.iter().map(|s| s.as_str()).collect();
    TestProject::new("share_reducer_loop")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &element_refs)
        .array_stock("pop[Region]", "100", &["update"], &[], None)
        .array_aux("share[Region]", "pop / SUM(pop[*])")
        .array_flow("update[Region]", "share * 0.001", None)
        .build_datamodel()
}

/// The synthetic scalar agg node `$⁚ltm⁚agg⁚0` (subscript-free; the
/// `SUM(pop[*])` is a whole-extent reduce).
const SHARE_REDUCER_AGG: &str = "$\u{205A}ltm\u{205A}agg\u{205A}0";

/// Count, among `model_ltm_variables`' `loop_score` equations, how many
/// reference at least `min_petals` distinct `$⁚ltm⁚link_score⁚pop[<elem>]→agg`
/// factors -- i.e. how many recovered loops traverse the agg node at least
/// `min_petals` times. A single-petal elementary circuit references exactly
/// one such factor; a k-petal combined loop references k.
fn count_loops_through_agg(ltm: &super::LtmVariablesResult, min_petals: usize) -> usize {
    let agg_factor_prefix = "$\u{205A}ltm\u{205A}link_score\u{205A}pop[";
    let agg_factor_suffix = format!("\u{2192}{SHARE_REDUCER_AGG}");
    ltm.vars
        .iter()
        .filter(|v| v.name.contains("\u{205A}loop_score\u{205A}"))
        .filter(|v| {
            let eq = v.equation.source_text();
            let distinct: std::collections::HashSet<String> = extract_quoted_refs(&eq)
                .into_iter()
                .filter(|r| r.starts_with(agg_factor_prefix) && r.ends_with(&agg_factor_suffix))
                .collect();
            distinct.len() >= min_petals
        })
        .count()
}

/// AC5.1: a reducer in a feedback loop over a dimension with more disjoint
/// petals than the loop budget recovers a *non-empty, budgeted* set of
/// cross-element-through-aggregate loops (not zero, as the pre-#515 hard
/// `petals.len() > MAX_AGG_PETALS -> continue` drop produced for >8-element
/// dims), `LtmVariablesResult.agg_recovery_truncated` is `true`, and a
/// `CompilationDiagnostic` `Warning` naming the truncation, the budget, and
/// the truncated aggregate node is emitted. The fixture is tiny (5 elements
/// -- well under the 50-node auto-flip SCC gate); the loop budget is shrunk
/// to 3 via the test-only `AggLoopBudgetGuard` so the budget is what clips
/// (per docs/dev/rust.md#test-time-budgets -- never trip a real gate with a
/// giant fixture).
#[test]
fn cross_agg_loop_recovery_truncates_at_budget() {
    use crate::db::{CompilationDiagnostic, DiagnosticError, DiagnosticSeverity};

    const TEST_BUDGET: usize = 3;
    let project = share_reducer_loop_fixture(5);
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;

    // Hold the override for the whole test -- `model_ltm_variables` is salsa-
    // memoized, so a later call on this db would otherwise return the cached
    // tiny-budget result regardless of the override state.
    let _budget_guard = super::AggLoopBudgetGuard::new(TEST_BUDGET);
    let ltm = model_ltm_variables(&db, model, sync.project);

    assert!(
        ltm.agg_recovery_truncated,
        "with 5 disjoint petals and a budget of {TEST_BUDGET}, cross-agg loop \
         recovery must report truncation"
    );

    let recovered = count_loops_through_agg(ltm, 2);
    assert!(
        recovered >= 1,
        "the budget is a stop, not a skip: at least one cross-agg loop must be \
         recovered even when truncated (got {recovered})"
    );
    assert!(
        recovered <= TEST_BUDGET,
        "the recovered cross-agg loop count ({recovered}) must not exceed the \
         budget ({TEST_BUDGET})"
    );

    let diags = model_ltm_variables::accumulated::<CompilationDiagnostic>(&db, model, sync.project);
    // The single reducer `SUM(pop[*])` hoists to `$⁚ltm⁚agg⁚0`; with 5
    // disjoint petals through it and a budget of 3, the budget fires while
    // enumerating that one agg, so the Warning names it.
    let has_truncation_warning = diags.iter().any(|CompilationDiagnostic(d)| {
        d.severity == DiagnosticSeverity::Warning
            && matches!(
                &d.error,
                DiagnosticError::Assembly(msg)
                    if msg.contains("truncated")
                        && msg.contains(&TEST_BUDGET.to_string())
                        && msg.contains(SHARE_REDUCER_AGG)
            )
    });
    assert!(
        has_truncation_warning,
        "cross-agg loop truncation must emit a Warning mentioning truncation, \
         the budget ({TEST_BUDGET}), and the truncated agg ({SHARE_REDUCER_AGG}); \
         got: {:?}",
        diags.iter().map(|c| &c.0).collect::<Vec<_>>()
    );
}

/// AC5.3 (no regression): a model whose reducer-in-a-loop has 3 disjoint
/// petals (under the production budget) recovers exactly the 3 pairwise
/// combinations (`{p0,p1}`, `{p0,p2}`, `{p1,p2}`) plus the full 3-petal
/// subset -- one canonical loop per disjoint subset, 4 recovered loops --
/// with `agg_recovery_truncated == false` and no truncation `Warning`.
#[test]
fn cross_agg_loop_recovery_three_petals_no_truncation() {
    use crate::db::{CompilationDiagnostic, DiagnosticError, DiagnosticSeverity};

    let project = share_reducer_loop_fixture(3);
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let ltm = model_ltm_variables(&db, model, sync.project);

    assert!(
        !ltm.agg_recovery_truncated,
        "a 3-petal model is well under the production budget; recovery must not \
         report truncation"
    );

    // Recovered loops through the agg >= 2 times: the 3 two-petal pairs +
    // the one ordering of the full triple = 4.
    let two_or_more = count_loops_through_agg(ltm, 2);
    assert_eq!(
        two_or_more, 4,
        "3 disjoint petals must recover C(3,2)=3 two-petal loops + 1 three-petal \
         loop = 4 cross-agg loops; got {two_or_more}"
    );
    // Exactly one of those visits the agg 3 times (the full-subset loop).
    let three_petal = count_loops_through_agg(ltm, 3);
    assert_eq!(
        three_petal, 1,
        "the full 3-petal subset yields exactly one canonical loop; got {three_petal}"
    );

    let diags = model_ltm_variables::accumulated::<CompilationDiagnostic>(&db, model, sync.project);
    assert!(
        !diags.iter().any(|CompilationDiagnostic(d)| {
            d.severity == DiagnosticSeverity::Warning
                && matches!(&d.error, DiagnosticError::Assembly(msg) if msg.contains("truncated"))
        }),
        "no truncation Warning expected for a 3-petal model; got: {:?}",
        diags.iter().map(|c| &c.0).collect::<Vec<_>>()
    );
}

/// AC5.3 (no regression, at the slow-path loop-builder level): driving
/// `build_loops_from_tiered` on the 3-petal fixture, the recovered
/// cross-agg `Loop`s' link multisets and stock sets match what the pre-#515
/// per-subset enumeration produced for the same petal pairs -- one cyclic
/// ordering per 2-petal pair (the m=2 case the pre-fix code already
/// covered). We compare each recovered two-petal loop's `(from, to,
/// polarity)` link *multiset* and stock *set* against a fixture-derived
/// expectation rather than the exact `Vec` order, because the recovered
/// cyclic node sequence is now rotation-canonicalized via a deterministic
/// petal sort (the pre-fix code stitched petals in Johnson-enumeration
/// order); a loop and a rotation of it are the same loop (`assign_loop_ids`
/// keys on the rotation-invariant endpoint set).
#[test]
fn cross_agg_two_petal_loops_match_pre_fix_content() {
    use crate::common::Ident;
    use crate::ltm::LinkPolarity;
    use std::collections::BTreeSet;

    let project = share_reducer_loop_fixture(3);
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;

    let tiered = crate::db::model_loop_circuits_tiered(&db, model, sync.project);
    let var_graph = causal_graph_with_modules(&db, model, sync.project);
    let source_vars = model.variables(&db);
    let dm_dims = project_datamodel_dims(&db, sync.project);
    let (loops, truncated_aggs) = build_loops_from_tiered(
        tiered,
        &var_graph,
        source_vars,
        &db,
        sync.project,
        dm_dims.as_slice(),
        MAX_CROSS_AGG_LOOPS,
    );
    assert!(
        truncated_aggs.is_empty(),
        "3-petal fixture must not truncate; got {truncated_aggs:?}"
    );

    let agg_ident = Ident::<Canonical>::new(SHARE_REDUCER_AGG);
    let agg_visits = |l: &crate::ltm::Loop| l.links.iter().filter(|lk| lk.to == agg_ident).count();
    let two_petal_loops: Vec<&crate::ltm::Loop> =
        loops.iter().filter(|l| agg_visits(l) == 2).collect();
    assert_eq!(
        two_petal_loops.len(),
        3,
        "expected 3 two-petal recovered loops; got {}",
        two_petal_loops.len()
    );

    // For each two-petal loop, its directed-edge multiset must be exactly the
    // union of the two petals' edges. A petal for region r is the cyclic
    // sequence [agg, share[r], update[r], pop[r]] -> edges agg->share[r],
    // share[r]->update[r], update[r]->pop[r], pop[r]->agg. Link endpoint
    // forms: `build_element_subscripted_links` keeps the `[r]` on a
    // dimensioned-source side and (for the A2A `share→update` / `update→pop`
    // hops) drops it, keeping the dimensioned `to` slot; `pop[r]→agg` keeps
    // the `pop[r]` source and the bare agg `to`; `agg→share[r]` keeps the
    // bare agg source and the `share[r]` slot.
    let petal_edges = |r: &str| -> BTreeSet<(String, String)> {
        let agg = SHARE_REDUCER_AGG.to_string();
        [
            (agg.clone(), format!("share[{r}]")),
            (format!("share[{r}]"), format!("update[{r}]")),
            (format!("update[{r}]"), format!("pop[{r}]")),
            (format!("pop[{r}]"), agg.clone()),
        ]
        .into_iter()
        .collect()
    };
    for l in &two_petal_loops {
        let got: BTreeSet<(String, String)> = l
            .links
            .iter()
            .map(|lk| (lk.from.as_str().to_string(), lk.to.as_str().to_string()))
            .collect();
        // Which two regions does this loop cover? Read them off the
        // `pop[<r>]→agg` links.
        let regions: Vec<String> = l
            .links
            .iter()
            .filter(|lk| lk.to == agg_ident)
            .map(|lk| {
                let f = lk.from.as_str();
                let start = f.find('[').unwrap();
                let end = f.rfind(']').unwrap();
                f[start + 1..end].to_string()
            })
            .collect();
        assert_eq!(regions.len(), 2, "two-petal loop must touch two regions");
        let mut want: BTreeSet<(String, String)> = petal_edges(&regions[0]);
        want.extend(petal_edges(&regions[1]));
        assert_eq!(
            got, want,
            "recovered two-petal loop (regions {regions:?}) has link multiset {got:?}, \
             expected the union of the two petals' edges {want:?}"
        );
        // The stocks are the per-element `pop[r]` nodes, one per region.
        let got_stocks: BTreeSet<String> =
            l.stocks.iter().map(|s| s.as_str().to_string()).collect();
        let want_stocks: BTreeSet<String> = regions.iter().map(|r| format!("pop[{r}]")).collect();
        assert_eq!(
            got_stocks, want_stocks,
            "recovered two-petal loop stocks {got_stocks:?}, expected {want_stocks:?}"
        );
        // At the `build_loops_from_tiered` level the synthetic-agg hops are
        // still Unknown-polarity (the variable-level graph has no agg node);
        // `model_ltm_variables` patches them afterward via
        // `recover_agg_hop_polarities`. So a recovered cross-agg loop here is
        // Undetermined, and at least the two `pop[r]→agg` hops are Unknown.
        assert_eq!(
            l.polarity,
            crate::ltm::LoopPolarity::Undetermined,
            "before `recover_agg_hop_polarities`, an agg-traversing recovered loop \
             is Undetermined"
        );
        let n_unknown = l
            .links
            .iter()
            .filter(|lk| lk.polarity == LinkPolarity::Unknown)
            .count();
        assert!(
            n_unknown >= 2,
            "expected >= 2 Unknown agg hops; got {n_unknown}"
        );
    }
}

/// AC5.2 (GH #676): a 4-petal reducer-in-a-loop fixture recovers exactly ONE
/// loop per disjoint petal subset -- the 6 two-petal pairs + the 4
/// three-petal triples + the 1 full 4-petal subset = 11 loops. For a fixed
/// petal subset every cyclic ordering yields the SAME edge multiset (each
/// petal contributes its `agg→head`/internal/`tail→agg` edges regardless of
/// its position in the concatenation), and the loop score is a commutative
/// product over that multiset -- so additional orderings would be
/// dominance-indistinguishable duplicates that only waste the
/// `MAX_CROSS_AGG_LOOPS` budget. (The k=4 fixture, not k=3, is what would
/// surface multiple orderings if they were emitted: a 3-petal subset has
/// only one ordering class either way.)
#[test]
fn cross_agg_loop_recovery_four_petals_one_loop_per_subset() {
    let project = share_reducer_loop_fixture(4);
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let ltm = model_ltm_variables(&db, model, sync.project);

    assert!(
        !ltm.agg_recovery_truncated,
        "a 4-petal model is well under the production budget; recovery must not truncate"
    );

    // All recovered cross-agg loops (>= 2 distinct `pop[*]→agg` factors):
    // C(4,2)=6 two-petal + C(4,3)=4 three-petal + 1 four-petal = 11.
    let recovered = count_loops_through_agg(ltm, 2);
    assert_eq!(
        recovered, 11,
        "expected 6 (m=2) + 4 (m=3) + 1 (m=4) = 11 recovered cross-agg loops; got {recovered}"
    );
    // Loops through the agg >= 3 times: 4 three-petal + 1 four-petal = 5.
    assert_eq!(count_loops_through_agg(ltm, 3), 5);
    // Loops through the agg >= 4 times: only the single full-subset loop.
    assert_eq!(
        count_loops_through_agg(ltm, 4),
        1,
        "the full 4-petal subset must yield exactly one canonical loop"
    );

    // The full-subset loop is unique: exactly one `loop_score` equation
    // references all four `pop[r]→agg` factors.
    let four_petal_loop_scores: Vec<(&str, String)> = ltm
        .vars
        .iter()
        .filter(|v| v.name.contains("\u{205A}loop_score\u{205A}"))
        .filter(|v| {
            let eq = v.equation.source_text();
            extract_quoted_refs(&eq)
                .iter()
                .filter(|r| {
                    r.starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}pop[")
                        && r.ends_with(&format!("\u{2192}{SHARE_REDUCER_AGG}"))
                })
                .collect::<std::collections::HashSet<_>>()
                .len()
                >= 4
        })
        .map(|v| (v.name.as_str(), v.equation.source_text()))
        .collect();
    assert_eq!(
        four_petal_loop_scores.len(),
        1,
        "exactly one loop per disjoint petal subset; got: {four_petal_loop_scores:?}"
    );
}

/// Phase 5 / Phase 4 interaction: `recover_cross_agg_loops`' petal
/// extraction handles a *subscripted* (arrayed) synthetic agg node
/// consistently. `growth[D1,D2] = SUM(matrix[D1,*]) * 0.0001 + 1` hoists
/// the `SUM(matrix[D1,*])` sub-expression into an *arrayed* synthetic agg
/// `$⁚ltm⁚agg⁚0[d1]` (read slice `[Iterated(d1), Reduced]`, `result_dims ==
/// [D1]`) that broadcasts over `D2`; `mflow[D1,D2] = growth[D1,D2]` (Bare,
/// same-element) feeds `matrix[D1,D2]`. The per-`(D1,D2)` loop is
/// `matrix[d1,d2] → $⁚ltm⁚agg⁚0[d1] → growth[d1,d2] → mflow[d1,d2] →
/// matrix[d1,d2]`. For one `D1` row the two `D2` slots are *disjoint* petals
/// through `agg[d1]` (their `growth`/`mflow`/`matrix` nodes are all distinct
/// `D2` slots; only the subscripted `agg[d1]` is shared, and it is the agg
/// node, not an internal node), so `recover_cross_agg_loops` combines them
/// into a loop that visits `$⁚ltm⁚agg⁚0[d1]` *twice* -- using the
/// subscripted node consistently (so `is_synthetic_agg_name` must recognize
/// the subscripted form) -- and a loop through `agg[a]` is never combined
/// with one through `agg[b]` (different element-graph nodes). The reported
/// variable-level loops never surface the synthetic agg. (The broadcast-case
/// loop *score* is pinned end-to-end by
/// `ltm_array_agg::broadcast_agg_loop_scores_are_finite_and_sustained` --
/// this test only pins the structural recovery.)
#[test]
fn cross_agg_loop_recovery_handles_subscripted_agg_node() {
    use crate::common::Ident;

    let project = TestProject::new("subscripted_agg_recovery")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .array_stock("matrix[D1,D2]", "100", &["mflow"], &[], None)
        // `SUM(matrix[D1,*])` as a *sub-expression* of an A2A-over-(D1,D2)
        // body -> arrayed synthetic agg over D1, broadcast over D2.
        .array_aux("growth[D1,D2]", "SUM(matrix[D1,*]) * 0.0001 + 1")
        // `mflow` is the same-element diagonal of `growth`, closing the
        // per-(D1,D2) loop. The two D2 slots of one D1 row run through
        // disjoint `growth`/`mflow`/`matrix` nodes -> disjoint petals
        // through the (shared) `agg[d1]`.
        .array_flow("mflow[D1,D2]", "growth", None);

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;

    // The synthetic agg is arrayed over D1.
    let agg_nodes = crate::ltm_agg::enumerate_agg_nodes(&db, model, sync.project);
    let synthetic: Vec<_> = agg_nodes.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(synthetic.len(), 1, "expected one synthetic agg");
    assert_eq!(
        synthetic[0].result_dims,
        vec!["D1".to_string()],
        "SUM(matrix[D1,*]) as a sub-expression of an A2A-over-(D1,D2) body mints an arrayed agg over D1"
    );

    let tiered = crate::db::model_loop_circuits_tiered(&db, model, sync.project);
    let var_graph = causal_graph_with_modules(&db, model, sync.project);
    let source_vars = model.variables(&db);
    let dm_dims = project_datamodel_dims(&db, sync.project);
    let (loops, truncated_aggs) = build_loops_from_tiered(
        tiered,
        &var_graph,
        source_vars,
        &db,
        sync.project,
        dm_dims.as_slice(),
        MAX_CROSS_AGG_LOOPS,
    );
    assert!(truncated_aggs.is_empty(), "got {truncated_aggs:?}");

    // A recovered loop that visits a *subscripted* agg node twice, for each
    // D1 element. The agg node in the element graph is `$⁚ltm⁚agg⁚0[a]` /
    // `$⁚ltm⁚agg⁚0[b]`; a loop through `agg[a]` twice has `agg[a]` as a link
    // target twice and never touches `agg[b]`.
    for d1 in &["a", "b"] {
        let agg_node = Ident::<Canonical>::new(&format!("$\u{205A}ltm\u{205A}agg\u{205A}0[{d1}]"));
        let other = if *d1 == "a" { "b" } else { "a" };
        let other_node =
            Ident::<Canonical>::new(&format!("$\u{205A}ltm\u{205A}agg\u{205A}0[{other}]"));
        let combined: Vec<&crate::ltm::Loop> = loops
            .iter()
            .filter(|l| {
                let visits_this = l.links.iter().filter(|lk| lk.to == agg_node).count();
                let visits_other = l
                    .links
                    .iter()
                    .any(|lk| lk.to == other_node || lk.from == other_node);
                visits_this >= 2 && !visits_other
            })
            .collect();
        assert!(
            !combined.is_empty(),
            "expected a recovered loop visiting {} twice (and not the other D1 slot); \
             loops: {:?}",
            agg_node.as_str(),
            loops
                .iter()
                .map(|l| l
                    .links
                    .iter()
                    .map(|lk| (lk.from.as_str(), lk.to.as_str()))
                    .collect::<Vec<_>>())
                .collect::<Vec<_>>()
        );
        // Such a loop has exactly two D2 slots' worth of `matrix` for this D1
        // row.
        for l in &combined {
            let matrix_stocks: Vec<&str> = l
                .stocks
                .iter()
                .map(|s| s.as_str())
                .filter(|s| s.starts_with("matrix["))
                .collect();
            assert_eq!(
                matrix_stocks.len(),
                2,
                "a 2-petal loop through agg[{d1}] should have 2 matrix slots; got {matrix_stocks:?}"
            );
            assert!(
                matrix_stocks.iter().all(|s| s.contains(&format!("[{d1},"))),
                "the matrix slots must all be in D1 row {d1}; got {matrix_stocks:?}"
            );
        }
    }

    // The reported variable-level loops never surface the synthetic agg node.
    let ltm = model_ltm_variables(&db, model, sync.project);
    assert!(!ltm.agg_recovery_truncated);
    let detected = crate::db::model_detected_loops(&db, model, sync.project);
    for l in &detected.loops {
        assert!(
            l.variables
                .iter()
                .all(|v| !v.contains("\u{205A}agg\u{205A}")),
            "model_detected_loops should not surface synthetic agg nodes; got: {:?}",
            l.variables
        );
    }
}

/// GH #528: in the strict-prefix *broadcast* case -- an arrayed synthetic
/// agg over `[D1]` (`SUM(matrix[D1,*])` as a sub-expression of an A2A body
/// over `D1 x D2`) feeding `growth[D1,D2]` -- the per-target-element
/// `agg → growth` link-score EQUATION must subscript the agg ident by the
/// target element's projection onto the agg's `result_dims` axes
/// (`[d1·a]`), not by the full target tuple (`[d1·a, d2·x]` -- an
/// out-of-shape over-subscript of a 1-D agg, whose fragment fails to
/// compile and stubs the score to a constant 0). The link-score NAME and
/// the `Δsource` denominator were already projected; this pins the partial
/// BODY's pinning too.
#[test]
fn broadcast_agg_to_target_equation_projects_agg_subscript() {
    let project = TestProject::new("broadcast_agg_eqn")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .array_stock("matrix[D1,D2]", "100", &["mflow"], &[], None)
        .array_aux("growth[D1,D2]", "SUM(matrix[D1,*]) * 0.01 + 1")
        .array_flow("mflow[D1,D2]", "growth[D1,D2]", None);

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let ltm = model_ltm_variables(&db, model, sync.project);

    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    for (d1, d2) in [("a", "x"), ("a", "y"), ("b", "x"), ("b", "y")] {
        let name =
            format!("$\u{205A}ltm\u{205A}link_score\u{205A}{agg}[{d1}]\u{2192}growth[{d1},{d2}]");
        let var = ltm.vars.iter().find(|v| v.name == name).unwrap_or_else(|| {
            panic!(
                "expected agg→target link score {name:?}; synthetic vars: {:?}",
                ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
            )
        });
        let eq = var.equation.source_text();
        // The numerator's live agg reference is pinned to the projected
        // (qualified) slot...
        let projected = format!("\"{agg}\"[d1\u{B7}{d1}]");
        assert!(
            eq.contains(&projected),
            "{name}: the equation body must pin the agg to the projected slot {projected}; \
             got: {eq}"
        );
        // ...and never to the full (over-subscripting) target tuple.
        let over_subscript = format!("\"{agg}\"[d1\u{B7}{d1}, d2\u{B7}{d2}]");
        assert!(
            !eq.contains(&over_subscript),
            "{name}: the equation body must NOT over-subscript the 1-D agg with the full \
             target tuple {over_subscript} (GH #528); got: {eq}"
        );
    }
}

/// `model_ltm_implicit_module_refs` is the module-typed projection of
/// `model_ltm_implicit_var_info`: exactly the entries whose meta has
/// `is_module == true`, mapped to their sub-model names.
///
/// Why the projection exists: `compile_ltm_implicit_var_fragment` runs once
/// per LTM implicit variable, and a large arrayed model produces hundreds of
/// thousands of those (C-LEARN v77: ~145k PREVIOUS-helper auxes). Each run
/// merges the module-typed refs into its compilation context so
/// cross-references between module-typed implicit vars resolve, but scanning
/// the full implicit-var map inside every run made LTM compilation O(K^2)
/// in the implicit-var count and dominated C-LEARN's compile time. The
/// salsa-cached projection is computed once instead.
#[test]
fn test_ltm_implicit_module_refs_is_module_projection() {
    use crate::common::{Canonical, Ident};
    use salsa::Setter;
    use std::collections::HashMap;

    // A SMOOTH-in-a-feedback-loop model: its link-score equations wrap the
    // module's inputs/outputs in PREVIOUS(), so parsing them synthesizes
    // implicit helper variables.
    let project = datamodel::Project {
        name: "smooth_feedback".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![x_model(
            "main",
            vec![
                x_aux("goal", "100", None),
                x_stock("level", "50", &["adjustment"], &[], None),
                x_aux("smoothed_level", "SMTH1(level, 3)", None),
                x_aux("gap", "goal - smoothed_level", None),
                x_flow("adjustment", "gap / 5", None),
            ],
        )],
        source: None,
        ai_information: None,
    };

    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let info = model_ltm_implicit_var_info(&db, source_model, source_project);
    // Pre-condition: the model produces LTM implicit vars at all, so the
    // projection assertion below is not vacuous.
    assert!(
        !info.is_empty(),
        "SMOOTH feedback model should synthesize LTM implicit helper vars"
    );

    let refs = model_ltm_implicit_module_refs(&db, source_model, source_project);
    let expected: HashMap<Ident<Canonical>, Ident<Canonical>> = info
        .iter()
        .filter(|(_, meta)| meta.is_module)
        .filter_map(|(name, meta)| {
            meta.model_name
                .as_ref()
                .map(|mn| (Ident::new(name), Ident::new(mn.as_str())))
        })
        .collect();
    assert_eq!(
        *refs, expected,
        "module-refs projection must contain exactly the module-typed implicit vars"
    );
}

/// `model_ltm_var_name_index` maps each LTM synthetic variable's name to the
/// index of its first occurrence in `model_ltm_variables(..).vars`, mirroring
/// `vars.iter().find(|v| v.name == name)` semantics.
///
/// Fragment compilation resolves dependencies that may themselves be LTM
/// synthetic variables (an A2A loop score referencing link scores, etc.). A
/// linear scan over all LTM vars per dependency lookup is O(N) per lookup and
/// O(N^2) across a model's full LTM compile (C-LEARN: ~145k lookups over 6.7k
/// vars), so the index is built once and salsa-cached.
#[test]
fn test_ltm_var_name_index_matches_vars() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = feedback_loop_project();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let ltm = model_ltm_variables(&db, source_model, source_project);
    assert!(
        !ltm.vars.is_empty(),
        "feedback model should have LTM synthetic vars"
    );

    let index = model_ltm_var_name_index(&db, source_model, source_project);
    for (i, v) in ltm.vars.iter().enumerate() {
        let first_occurrence = ltm
            .vars
            .iter()
            .position(|other| other.name == v.name)
            .expect("var must find itself");
        assert_eq!(
            index.get(&v.name),
            Some(&first_occurrence),
            "index must map {} to its first occurrence (found at {i})",
            v.name,
        );
    }
    // Every index entry refers back to a var with that exact name.
    for (name, &i) in index.iter() {
        assert_eq!(
            &ltm.vars[i].name, name,
            "index entry for {name} must point at a var with that name"
        );
    }
}

// ── GH #486: LTM requires Euler integration ─────────────────────────────
//
// The 2023 flow-to-stock link-score formula
// (`PREVIOUS(flow) - PREVIOUS(PREVIOUS(flow))`) only aligns the numerator to
// the causal interval that drove the stock change from t-1 to t under Euler
// integration. RK2/RK4 sub-step the stock update, so that alignment breaks
// and the resulting link scores are mathematically meaningless. Non-Euler
// integration IS honored at runtime (the VM and wasm backends both have
// distinct RK2/RK4 stepping loops), so the wrong scores would look plausible
// but be wrong -- a silent-correctness hazard. The engine therefore rejects
// the combination at sim-compile time.
//
// CRITICAL granularity (the multi-model probes below): the VM has a SINGLE
// global integration method, taken from the MAIN (root) model's
// `model_sim_specs` override else the project specs -- a submodel's own
// `model_sim_specs` is NEVER consulted by the VM (`assemble_simulation`'s
// `Specs` selection). So the guard must resolve and apply that one
// main-governed method, not each model's own specs.

/// Build a single-stock feedback-loop project (population/births/birth_rate)
/// with the requested integration method, so the GH #486 guard can be
/// exercised under each non-Euler method.
#[cfg(test)]
fn feedback_loop_project_with_method(method: datamodel::SimMethod) -> datamodel::Project {
    let mut project = feedback_loop_project();
    project.sim_specs.sim_method = method;
    project
}

/// The substring every GH #486 rejection must carry so users understand the
/// Euler assumption behind it.
#[cfg(test)]
const LTM_EULER_DIAGNOSTIC_MARKER: &str = "Euler";

/// Whether compiling `main` with LTM enabled fails with the GH #486
/// Euler-assumption error. Returns `Some(details)` on the expected rejection,
/// `None` when the compile succeeds (so the same probe pins both directions).
#[cfg(test)]
fn ltm_euler_rejection(db: &SimlinDb, source_project: SourceProject) -> Option<String> {
    match compile_project_incremental(db, source_project, "main") {
        Ok(_) => None,
        Err(err) => {
            let details = err.details.unwrap_or_default();
            assert!(
                details.contains(LTM_EULER_DIAGNOSTIC_MARKER),
                "an LTM compile failure here must be the Euler rejection: {details}"
            );
            Some(details)
        }
    }
}

#[test]
fn test_ltm_with_rk4_fails_sim_compilation() {
    // `simlin_sim_new` and `simlin_project_get_errors` both build the sim via
    // `compile_project_incremental`; the Euler rejection rides its `Err`.
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = feedback_loop_project_with_method(datamodel::SimMethod::RungeKutta4);
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);

    let details =
        ltm_euler_rejection(&db, source_project).expect("LTM + RK4 must fail sim compilation");
    assert!(
        details.contains("RK4") || details.contains("Runge"),
        "the error should name the offending integration method: {details}"
    );
}

#[test]
fn test_ltm_with_rk2_fails_sim_compilation() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = feedback_loop_project_with_method(datamodel::SimMethod::RungeKutta2);
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);

    assert!(
        ltm_euler_rejection(&db, source_project).is_some(),
        "LTM + RK2 must fail sim compilation"
    );
}

#[test]
fn test_ltm_with_euler_compiles_clean() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = feedback_loop_project_with_method(datamodel::SimMethod::Euler);
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);

    assert!(
        ltm_euler_rejection(&db, source_project).is_none(),
        "LTM + Euler is the supported combination and must compile"
    );
}

#[test]
fn test_rk4_without_ltm_compiles_clean() {
    // The guard fires ONLY when LTM is actually requested. The same non-Euler
    // model with LTM disabled compiles and simulates as before.
    let db = SimlinDb::default();
    let project = feedback_loop_project_with_method(datamodel::SimMethod::RungeKutta4);
    let source_project = sync_from_datamodel(&db, &project).project;
    // ltm_enabled defaults to false on a freshly-synced project.

    assert!(
        compile_project_incremental(&db, source_project, "main").is_ok(),
        "RK4 model without LTM should compile"
    );
}

/// The Euler rejection must reach `simlin_project_get_errors`. That FFI path
/// compiles `main` via `compile_project_incremental` and turns the resulting
/// `Err` into a formatted error (the `vm_error` channel in
/// `gather_error_details_with_db`), so asserting against the compile `Err`
/// covers the production diagnostics surface. (The per-model accumulator path
/// can't carry this diagnostic correctly: `model_all_diagnostics` has no
/// main-model concept, and the method is main-governed.)
#[test]
fn test_ltm_rk4_euler_error_reaches_compile_path() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = feedback_loop_project_with_method(datamodel::SimMethod::RungeKutta4);
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);

    assert!(
        ltm_euler_rejection(&db, source_project).is_some(),
        "the GH #486 Euler error must surface through compile_project_incremental"
    );
}

/// A user-defined submodel with an internal stock + flow feedback loop and an
/// output port. Used by the multi-model probes: the submodel's own
/// `model_sim_specs` is set by the caller to assert the VM ignores it.
#[cfg(test)]
fn feedback_submodel(sim_specs: Option<datamodel::SimSpecs>) -> datamodel::Model {
    let mut m = x_model(
        "sub",
        vec![
            x_aux("input", "0", None),
            x_stock("level", "50", &["adjust"], &[], None),
            x_flow("adjust", "(input - level) / 5", None),
            x_aux("output", "level", None),
        ],
    );
    m.sim_specs = sim_specs;
    m
}

/// GH #486 false-negative probe (the silent #486 hazard): the MAIN model has
/// no stock and overrides the integration method to RK4, while the submodel it
/// instantiates holds the stock+flow feedback loop and has no override. The VM
/// integrates EVERYTHING under the main-governed RK4, so the submodel's
/// flow-to-stock link score is produced and run -- and must be rejected. A
/// per-submodel check (the bug) would miss it: the submodel falls back to the
/// project's Euler.
#[test]
fn test_ltm_main_rk4_submodel_stock_is_rejected() {
    use salsa::Setter;

    let mut main = x_model(
        "main",
        vec![
            x_module("sub", &[("driver", "input")], None),
            x_aux("driver", "1", None),
            x_aux("observed", "sub.output", None),
        ],
    );
    // Main overrides the integration method to RK4; this is what the VM honors.
    main.sim_specs = Some(datamodel::SimSpecs {
        sim_method: datamodel::SimMethod::RungeKutta4,
        ..feedback_loop_project().sim_specs
    });

    let mut project = feedback_loop_project();
    project.models = vec![main, feedback_submodel(None)];
    // Project default stays Euler -- the trap the buggy per-submodel resolution
    // falls into.
    project.sim_specs.sim_method = datamodel::SimMethod::Euler;

    let mut db = SimlinDb::default();
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);

    let details = ltm_euler_rejection(&db, source_project)
        .expect("main-RK4 + submodel-stock LTM must be rejected (the #486 hazard)");
    assert!(
        details.contains("RK4") || details.contains("Runge"),
        "the rejection must name the main-governed method: {details}"
    );
}

/// GH #486 false-positive probe: the submodel overrides the integration method
/// to RK4, but the MAIN model (and project) are Euler. The VM runs Euler (main
/// governs); the submodel's RK4 override is dead. The scores are valid, so the
/// compile must SUCCEED. A per-submodel check (the bug) would wrongly reject.
#[test]
fn test_ltm_submodel_rk4_override_main_euler_is_accepted() {
    use salsa::Setter;

    // Main holds its own stock+flow feedback loop (so LTM produces scores) and
    // also instantiates the RK4-overriding submodel.
    let main = x_model(
        "main",
        vec![
            x_stock("population", "100", &["births"], &[], None),
            x_flow("births", "population * birth_rate", None),
            x_aux("birth_rate", "0.1", None),
            x_module("sub", &[("births", "input")], None),
            x_aux("observed", "sub.output", None),
        ],
    );
    // The submodel overrides to RK4 -- which the VM IGNORES (main governs).
    let sub_specs = datamodel::SimSpecs {
        sim_method: datamodel::SimMethod::RungeKutta4,
        ..feedback_loop_project().sim_specs
    };

    let mut project = feedback_loop_project();
    project.models = vec![main, feedback_submodel(Some(sub_specs))];
    project.sim_specs.sim_method = datamodel::SimMethod::Euler;

    let mut db = SimlinDb::default();
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);

    let compiled = compile_project_incremental(&db, source_project, "main");
    assert!(
        compiled.is_ok(),
        "main-Euler + submodel-RK4-override must compile (VM runs Euler): {:?}",
        compiled.err()
    );
    // And it actually simulates under Euler.
    let mut vm = crate::vm::Vm::new(compiled.unwrap()).expect("VM creation should succeed");
    vm.run_to_end().expect("Euler sim should run to completion");
}

// ── GH #663: the non-Euler guard must not over-reject loop-free models ───
//
// #486's guard rejected RK2/RK4 + LTM whenever ANY instantiated model had a
// stock. But LTM only emits a flow-to-stock link score for a stock that
// participates in a feedback loop; a stock in NO loop (an open-loop
// accumulation -- a constant inflow that never reads the stock back) produces
// zero flow-to-stock scores, so the non-Euler method has nothing to corrupt
// and the rejection is a false positive. The refined guard fires only when an
// instantiated model has a feedback loop (which, in a well-formed SD model,
// necessarily passes through a stock).

/// An open-loop accumulation model: a stock fed by a CONSTANT inflow. The
/// inflow `rate` does not read the stock back, so there is no feedback loop
/// and LTM emits no flow-to-stock link score for `tank`. With the requested
/// integration method on the project specs.
#[cfg(test)]
fn open_loop_project_with_method(method: datamodel::SimMethod) -> datamodel::Project {
    let mut project = feedback_loop_project();
    project.sim_specs.sim_method = method;
    project.models = vec![x_model(
        "main",
        vec![
            // `tank` accumulates `rate`, but nothing reads `tank` -> no cycle.
            x_stock("tank", "0", &["rate"], &[], None),
            x_flow("rate", "fill_rate", None),
            x_aux("fill_rate", "5", None),
        ],
    )];
    project
}

#[test]
fn test_ltm_rk4_open_loop_stock_compiles() {
    // The RED case for GH #663: a stock with no feedback loop under RK4 + LTM
    // emits no flow-to-stock scores, so the non-Euler guard must NOT reject it.
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = open_loop_project_with_method(datamodel::SimMethod::RungeKutta4);
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);

    let compiled = compile_project_incremental(&db, source_project, "main");
    assert!(
        compiled.is_ok(),
        "RK4 + LTM on a loop-free (open-loop accumulation) model must compile: {:?}",
        compiled.err()
    );
    // And it simulates -- the stock genuinely accumulates under RK4.
    let mut vm = crate::vm::Vm::new(compiled.unwrap()).expect("VM creation should succeed");
    vm.run_to_end().expect("RK4 sim should run to completion");
}

#[test]
fn test_ltm_rk2_open_loop_stock_compiles() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = open_loop_project_with_method(datamodel::SimMethod::RungeKutta2);
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);

    assert!(
        compile_project_incremental(&db, source_project, "main").is_ok(),
        "RK2 + LTM on a loop-free model must compile"
    );
}

#[test]
fn test_ltm_euler_open_loop_stock_compiles() {
    // The Euler control: a loop-free model under Euler always compiled and
    // still must -- this pins that the refined guard didn't change the Euler
    // path or break loop-free models generally.
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = open_loop_project_with_method(datamodel::SimMethod::Euler);
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);

    assert!(
        compile_project_incremental(&db, source_project, "main").is_ok(),
        "Euler + LTM on a loop-free model must compile"
    );
}

/// The soundness corner of GH #663: an open-loop stock has NO feedback loop,
/// but DISCOVERY mode (here user-forced) scores EVERY causal edge -- including
/// the open-loop `rate → tank` flow-to-stock edge. That flow-to-stock score is
/// exactly the Euler-only formula, so under RK4 the model must STILL be
/// rejected even though it has no loop. A guard that keyed on "has any feedback
/// loop" would wrongly accept this (the stock is in no loop); the guard must
/// key on "actually emits a flow-to-stock score".
#[test]
fn test_ltm_rk4_open_loop_stock_in_discovery_mode_is_rejected() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = open_loop_project_with_method(datamodel::SimMethod::RungeKutta4);
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);
    // Force discovery mode: now ALL edges get link scores, so the open-loop
    // stock's flow-to-stock edge IS scored.
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let details = ltm_euler_rejection(&db, source_project).expect(
        "RK4 + LTM + forced-discovery on an open-loop stock must be rejected: discovery \
         scores the flow-to-stock edge",
    );
    assert!(
        details.contains("RK4") || details.contains("Runge"),
        "the rejection must name the offending method: {details}"
    );
}

/// The Euler control for the forced-discovery open-loop case: Euler is the
/// supported integration, so even though a flow-to-stock score IS emitted, the
/// combination compiles. Pins that the discovery-mode rejection is gated on the
/// non-Euler method, not on the mere emission of a flow-to-stock score.
#[test]
fn test_ltm_euler_open_loop_stock_in_discovery_mode_compiles() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = open_loop_project_with_method(datamodel::SimMethod::Euler);
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    assert!(
        ltm_euler_rejection(&db, source_project).is_none(),
        "Euler + LTM + forced-discovery on an open-loop stock must compile"
    );
}

/// An ARRAYED open-loop stock with a DECORATED flow-to-stock `to` endpoint. A
/// scalar flow `rate` (a constant, never reading the stock back) broadcasts into
/// an arrayed stock `tank[D]`, so there is no feedback loop through any element.
/// Under RK4 + LTM with FORCED DISCOVERY, discovery scores every causal edge, so
/// a flow-to-stock score IS emitted for the `rate → tank` edge -- and because
/// the target is arrayed, the score is emitted PER ELEMENT with a decorated `to`
/// endpoint: `$⁚ltm⁚link_score⁚rate→tank[a]`, `…→tank[b]`, `…→tank[c]`. The bare
/// stock set is `{"tank"}`, so the guard must strip the `[a]` element decoration
/// off the `to` endpoint before matching. This pins the full decode chain on a
/// decorated `to` endpoint: `link_score_edge_endpoints` parses the name,
/// `strip_subscript` strips the element decoration, and `stocks.contains`
/// matches the bare stock base name. A guard that compared the decorated `to`
/// endpoint (`"tank[a]"`) against the bare stock set (`{"tank"}`) directly would
/// miss the arrayed score entirely and wrongly accept the non-Euler model.
#[test]
fn test_ltm_rk4_arrayed_open_loop_stock_in_discovery_mode_is_rejected() {
    use salsa::Setter;

    let project = TestProject::new("arrayed_open_loop")
        .with_sim_method(datamodel::SimMethod::RungeKutta4)
        .named_dimension("D", &["a", "b", "c"])
        // `tank[D]` accumulates the scalar `rate` (broadcast across D), but
        // `rate` is a constant that never reads `tank` -> no cycle. The arrayed
        // target makes the flow-to-stock score per-element, so the `to` endpoint
        // carries an `[a]`/`[b]`/`[c]` decoration the guard must strip.
        .array_stock("tank[D]", "0", &["rate"], &[], None)
        .flow("rate", "fill_rate", None)
        .aux("fill_rate", "5", None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);
    // Force discovery mode: ALL edges get link scores, so the arrayed open-loop
    // stock's `rate → tank[d]` flow-to-stock edges ARE scored (per element).
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let details = ltm_euler_rejection(&db, source_project).expect(
        "RK4 + LTM + forced-discovery on an ARRAYED open-loop stock must be rejected: \
         discovery scores the per-element flow-to-stock edge with a decorated `to` endpoint",
    );
    assert!(
        details.contains("RK4") || details.contains("Runge"),
        "the rejection must name the offending method: {details}"
    );
}

/// A stock-free RK4 main that instantiates a submodel whose stock is in NO
/// internal loop, but which the parent reads through an input/output port.
/// A module with input ports scores ALL its causal edges (the pathway/composite
/// machinery needs them), so `model_ltm_variables` emits the submodel's
/// `fill → level` flow-to-stock score even though `level` is in no loop. That
/// score is the Euler-only formula, so under the main-governed RK4 the model
/// must be rejected. This pins that the refined guard reads the EMITTED scores
/// (a loop-presence proxy would wrongly accept this -- the submodel stock has
/// no loop).
#[test]
fn test_ltm_main_rk4_input_port_submodel_stock_is_rejected() {
    use salsa::Setter;

    let mut main = x_model(
        "main",
        vec![
            x_module("sub", &[("driver", "input")], None),
            x_aux("driver", "1", None),
            x_aux("observed", "sub.output", None),
        ],
    );
    main.sim_specs = Some(datamodel::SimSpecs {
        sim_method: datamodel::SimMethod::RungeKutta4,
        ..feedback_loop_project().sim_specs
    });

    // `level` accumulates `fill` (= the `input` port), never read back, so the
    // submodel has no internal feedback loop -- but its input port forces
    // all-edge scoring, which emits the `fill → level` flow-to-stock score.
    let mut sub = x_model(
        "sub",
        vec![
            x_aux("input", "0", None),
            x_stock("level", "0", &["fill"], &[], None),
            x_flow("fill", "input", None),
            x_aux("output", "level", None),
        ],
    );
    sub.sim_specs = None;

    let mut project = feedback_loop_project();
    project.models = vec![main, sub];
    project.sim_specs.sim_method = datamodel::SimMethod::Euler;

    let mut db = SimlinDb::default();
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);

    let details = ltm_euler_rejection(&db, source_project).expect(
        "main-RK4 + input-port submodel-stock LTM must be rejected: an input-port submodel \
         scores its flow-to-stock edge even without a loop",
    );
    assert!(
        details.contains("RK4") || details.contains("Runge"),
        "the rejection must name the main-governed method: {details}"
    );
}

/// The same stock-free RK4 main, but the submodel's stock IS in a feedback
/// loop. A flow-to-stock score IS emitted for the submodel instance, so the
/// refined guard must STILL reject. (This is the #663 analogue of the existing
/// #486 `test_ltm_main_rk4_submodel_stock_is_rejected` false-negative probe.)
#[test]
fn test_ltm_main_rk4_looped_submodel_is_rejected() {
    use salsa::Setter;

    let mut main = x_model(
        "main",
        vec![
            x_module("sub", &[("driver", "input")], None),
            x_aux("driver", "1", None),
            x_aux("observed", "sub.output", None),
        ],
    );
    main.sim_specs = Some(datamodel::SimSpecs {
        sim_method: datamodel::SimMethod::RungeKutta4,
        ..feedback_loop_project().sim_specs
    });

    let mut project = feedback_loop_project();
    // `feedback_submodel` has the internal `level <-> adjust` feedback loop.
    project.models = vec![main, feedback_submodel(None)];
    project.sim_specs.sim_method = datamodel::SimMethod::Euler;

    let mut db = SimlinDb::default();
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);

    let details = ltm_euler_rejection(&db, source_project)
        .expect("main-RK4 + looped submodel-stock LTM must be rejected");
    assert!(
        details.contains("RK4") || details.contains("Runge"),
        "the rejection must name the main-governed method: {details}"
    );
}

// ---------------------------------------------------------------------------
// GH #738: scalar-target agg over an array *expression* (`SUM(pop[*] * scale)`)
// ---------------------------------------------------------------------------

/// Build a GH #738-shaped fixture: a *scalar* flow whose equation embeds a
/// reducer over an array expression (`grow_eqn`, e.g. `1 + SUM(pop[*] *
/// scale)`), with the scalar feeder `scale` closing a feedback loop through
/// the scalar stock `total`. The `1 +` keeps the reducer a sub-expression,
/// so it is hoisted into a synthetic `$⁚ltm⁚agg⁚{n}` whose own equation is
/// the reducer text -- the fragment that GH #738's empty-lowering-scope bug
/// rejected. `pop2` and `matrix` exist so equation variants can exercise
/// multi-source (`SUM(pop[*] + pop2[*])`) and pinned-slice
/// (`SUM(matrix[north,*] * scale)`) reducers against the same model.
fn build_scalar_target_array_expr_reducer_project(
    name: &str,
    grow_eqn: &str,
) -> datamodel::Project {
    TestProject::new(name)
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .named_dimension("band", &["lo", "hi"])
        .array_stock("pop[region]", "100", &["pgrow"], &[], None)
        .array_flow("pgrow[region]", "pop[region] * 0.05", None)
        .array_aux("pop2[region]", "pop[region] * 2")
        .array_aux("matrix[region,band]", "pop[region] * 0.5")
        .scalar_aux("scale", "0.001 * total + 0.01")
        .stock("total", "100", &["grow"], &[], None)
        .flow("grow", grow_eqn, None)
        .build_datamodel()
}

/// Collect the LTM fragment-failure warnings for `project`, asserting first
/// that the expected synthetic agg was actually hoisted (so a test using
/// this helper can never pass vacuously because no agg node existed).
fn ltm_fragment_failures_with_agg_present(project: &datamodel::Project) -> Vec<String> {
    use crate::db::collect_model_diagnostics;
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let ltm = model_ltm_variables(&db, source_model, source_project);
    assert!(
        ltm.vars
            .iter()
            .any(|v| v.name.contains("\u{205A}agg\u{205A}")),
        "the inlined reducer must be hoisted into a synthetic agg; \
         synthetic vars: {:?}",
        ltm.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );

    collect_model_diagnostics(&db, source_model, source_project)
        .iter()
        .filter(|d| is_ltm_fragment_failure(d))
        .map(|d| format!("{:?}: {:?}", d.variable, d.error))
        .collect()
}

/// Assert GH #738's contract on a scalar-target reducer fixture: the
/// synthetic `$⁚ltm⁚agg⁚{n}` fragment compiles (no fragment-failure
/// Warning names an agg variable), and the only failures still present --
/// if any -- are the known residual: the *direct* `scale→grow` link score,
/// whose ceteris-paribus partial contains `PREVIOUS(pop[*])` and trips the
/// documented GH #541 scalar-helper limitation (`make_temp_arg` outside A2A
/// context captures a wildcard-subscripted arrayed arg into an ill-typed
/// *scalar* helper). That residual is a separate defect from #738 -- the
/// loop score consuming that direct link stays 0 pending GH #737's
/// routing fix anyway. The second assertion TOLERATES the residual rather
/// than REQUIRING it: any unexpected failure still trips it, but fixing
/// the #541-class gap later leaves it passing (an empty failure list
/// satisfies the `all`), so the assertion cannot rot.
fn assert_agg_fragments_compile(eqn: &str, failures: &[String]) {
    assert!(
        failures.iter().all(|f| !f.contains("\u{205A}agg\u{205A}")),
        "the synthetic agg fragment for the scalar-target {eqn:?} model \
         must compile; failures: {failures:?}"
    );
    assert!(
        failures.iter().all(|f| f.contains("scale\u{2192}grow")),
        "the only tolerated residual fragment failure for {eqn:?} is the \
         direct scale\u{2192}grow link score (the PREVIOUS-of-wildcard \
         scalar-helper gap); failures: {failures:?}"
    );
}

/// GH #738: the synthetic agg hoisted for `SUM(pop[*] * scale)` with a
/// *scalar* target must compile. Before the fix,
/// `compile_ltm_equation_fragment` lowered the agg's equation with an empty
/// `ScopeStage0.models`, so the `pop[*] * scale` Op2 never got its Expr2
/// `ArrayBounds` and Pass-1 temp decomposition never hoisted it -- codegen
/// then rejected the inline Op2 under SUM and the agg was silently stubbed
/// to a constant 0, corrupting every score routed through it.
#[test]
fn scalar_target_agg_over_array_expression_fragments_compile() {
    let eqn = "1 + SUM(pop[*] * scale)";
    let project =
        build_scalar_target_array_expr_reducer_project("scalar_target_array_expr_agg", eqn);
    let failures = ltm_fragment_failures_with_agg_present(&project);
    assert_agg_fragments_compile(eqn, &failures);
}

/// GH #738, adjacent shapes: the fix is general to the scalar-target
/// reducer-over-array-expression class, not just `SUM(arrayed * scalar)`.
/// Each variant hoists a synthetic agg whose equation embeds an array
/// expression: a different reducer kind (`MEAN`/`MIN`/`MAX`), a
/// multi-source reducer (`SUM(a[*] + b[*])`), and a pinned-slice reducer
/// (`SUM(matrix[north,*] * scale)`).
#[test]
fn scalar_target_reducer_variants_fragments_compile() {
    for (tag, eqn) in [
        ("mean", "1 + MEAN(pop[*] * scale)"),
        ("min", "1 + MIN(pop[*] * scale)"),
        ("max", "1 + MAX(pop[*] * scale)"),
        ("multi_source", "1 + SUM(pop[*] + pop2[*])"),
        ("pinned_slice", "1 + SUM(matrix[north,*] * scale)"),
    ] {
        let project = build_scalar_target_array_expr_reducer_project(
            &format!("scalar_target_variant_{tag}"),
            eqn,
        );
        let failures = ltm_fragment_failures_with_agg_present(&project);
        assert_agg_fragments_compile(eqn, &failures);
    }
}
