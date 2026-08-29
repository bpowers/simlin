// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Incrementality and ownership gates for the per-variable unit-analysis input.

use super::*;
use crate::db::exec_probe::ProbedDb;
use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module, x_project};

fn scalar_project(alpha_eqn: &str, extra: bool) -> crate::datamodel::Project {
    let mut variables = vec![
        x_aux("alpha", alpha_eqn, Some("widget")),
        x_aux("beta", "2", Some("widget")),
    ];
    if extra {
        variables.push(x_aux("unrelated", "3", Some("widget")));
    }
    x_project(sim_specs_with_units("month"), &[x_model("main", variables)])
}

#[test]
fn units_and_fragments_share_one_lowered_source_variable_memo() {
    let mut probed = ProbedDb::new();
    let project = scalar_project("1", false);
    let state = sync_from_datamodel_incremental(probed.db_mut(), &project, None);
    let sync = state.to_sync_result();
    let main = sync.models["main"].source;

    probed.reset();
    super::units::check_model_units(probed.db(), main, sync.project);
    let after_units = probed.counts();
    assert_eq!(
        after_units.get("lowered_source_variable"),
        Some(&(2, 2)),
        "unit analysis must lower each source variable once through the production projection"
    );

    let alpha = sync.models["main"].variables["alpha"].source;
    let lowered = lowered_source_variable(probed.db(), alpha, main, sync.project);
    let explicit =
        super::var_fragment::explicit_fragment_input(probed.db(), alpha, main, sync.project, &[]);
    let fragment_input = explicit.input.unwrap_or_else(|| {
        panic!(
            "production fragment input failed: {:?}",
            explicit.diagnostics
        )
    });
    assert!(
        matches!(fragment_input.target, std::borrow::Cow::Borrowed(_)),
        "the explicit fragment must borrow, rather than clone, the lowering memo"
    );
    assert!(
        std::sync::Arc::ptr_eq(fragment_input.target.as_ref(), lowered),
        "unit analysis and fragment compilation must observe the same lowered result handle"
    );

    compile_var_fragment(
        probed.db(),
        alpha,
        main,
        sync.project,
        ModuleInputSet::empty(probed.db()),
    );
    assert_eq!(
        probed.counts().get("lowered_source_variable"),
        Some(&(2, 2)),
        "fragment compilation must reuse the exact lowered value unit analysis demanded"
    );
}

#[test]
fn model_graph_maps_share_every_per_variable_lowering_payload() {
    let project = x_project(
        sim_specs_with_units("month"),
        &[x_model(
            "main",
            vec![
                x_aux("alpha", "1", Some("widget")),
                x_aux("smoothed", "SMTH1(alpha, 2)", Some("widget")),
            ],
        )],
    );
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let graph_variables = model_lowered_variables(&db, model, sync.project);

    for (name, source_var) in model.variables(&db) {
        let lowered = lowered_source_variable(&db, *source_var, model, sync.project);
        let graph_value = graph_variables
            .get(&Ident::new(name))
            .unwrap_or_else(|| panic!("graph map contains explicit variable {name}"));
        assert!(
            std::sync::Arc::ptr_eq(graph_value, lowered),
            "{name}: the graph map must clone the memo handle, never the Expr2 payload"
        );
    }

    for name in model_implicit_var_info(&db, model, sync.project).keys() {
        let lowered = lowered_implicit_variable(&db, model, sync.project, name.clone())
            .as_ref()
            .unwrap_or_else(|| panic!("production helper {name} lowers"));
        let graph_value = graph_variables
            .get(&Ident::new(name))
            .unwrap_or_else(|| panic!("graph map contains implicit variable {name}"));
        assert!(
            std::sync::Arc::ptr_eq(graph_value, lowered),
            "{name}: the graph map must share the implicit lowering memo payload"
        );
    }
}

#[test]
fn graph_map_rebuilds_handles_without_relowering_unchanged_payloads() {
    let mut probed = ProbedDb::new();
    let initial = scalar_project("1", false);
    let state1 = sync_from_datamodel_incremental(probed.db_mut(), &initial, None);
    let sync1 = state1.to_sync_result();
    model_lowered_variables(probed.db(), sync1.models["main"].source, sync1.project);

    let edited = scalar_project("beta", false);
    probed.reset();
    let state2 = sync_from_datamodel_incremental(probed.db_mut(), &edited, Some(&state1));
    let sync2 = state2.to_sync_result();
    model_lowered_variables(probed.db(), sync2.models["main"].source, sync2.project);
    assert_eq!(
        probed.counts().get("lowered_source_variable"),
        Some(&(1, 1)),
        "an equation edit may lower only that payload while rebuilding the Arc map"
    );

    let added = scalar_project("beta", true);
    probed.reset();
    let state3 = sync_from_datamodel_incremental(probed.db_mut(), &added, Some(&state2));
    let sync3 = state3.to_sync_result();
    model_lowered_variables(probed.db(), sync3.models["main"].source, sync3.project);
    assert_eq!(
        probed.counts().get("lowered_source_variable"),
        Some(&(1, 1)),
        "adding a variable may lower only the new payload while rebuilding the Arc map"
    );
}

#[test]
fn a_scalar_edit_does_not_lower_unrelated_variables() {
    let mut probed = ProbedDb::new();
    let initial = scalar_project("1", false);
    let state1 = sync_from_datamodel_incremental(probed.db_mut(), &initial, None);
    let sync1 = state1.to_sync_result();
    super::units::check_model_units(probed.db(), sync1.models["main"].source, sync1.project);

    let edited = scalar_project("beta", false);
    probed.reset();
    let state2 = sync_from_datamodel_incremental(probed.db_mut(), &edited, Some(&state1));
    let sync2 = state2.to_sync_result();
    super::units::check_model_units(probed.db(), sync2.models["main"].source, sync2.project);

    assert_eq!(
        probed.counts().get("lowered_source_variable"),
        Some(&(1, 1)),
        "only the edited alpha variable may execute the Expr2 lowering query"
    );
}

fn nested_macro_project(
    macro_name: &str,
    include_call: bool,
    include_unrelated: bool,
) -> crate::datamodel::Project {
    let mut main_variables = vec![x_aux("input", "1", Some("widget"))];
    if include_call {
        let call = format!("{macro_name}(input)");
        main_variables.push(x_aux("called", &call, Some("widget")));
    }
    if include_unrelated {
        main_variables.push(x_aux("unrelated", "4", Some("widget")));
    }
    let mut macro_model = x_model(
        macro_name,
        vec![
            x_aux("arg", "0", Some("widget")),
            x_aux("output", "nested_macro(arg)", Some("widget")),
        ],
    );
    macro_model.macro_spec = Some(crate::datamodel::MacroSpec {
        parameters: vec!["arg".to_string()],
        primary_output: "output".to_string(),
        additional_outputs: vec![],
    });
    let mut nested_macro = x_model(
        "nested_macro",
        vec![
            x_aux("nested_arg", "0", Some("widget")),
            x_aux("nested_output", "nested_arg", Some("widget")),
        ],
    );
    nested_macro.macro_spec = Some(crate::datamodel::MacroSpec {
        parameters: vec!["nested_arg".to_string()],
        primary_output: "nested_output".to_string(),
        additional_outputs: vec![],
    });
    x_project(
        sim_specs_with_units("month"),
        &[x_model("main", main_variables), macro_model, nested_macro],
    )
}

#[test]
fn unrelated_variable_addition_backdates_the_lightweight_module_scope() {
    let mut probed = ProbedDb::new();
    let initial = nested_macro_project("renamed_macro", true, false);
    let state1 = sync_from_datamodel_incremental(probed.db_mut(), &initial, None);
    let sync1 = state1.to_sync_result();
    let main1 = sync1.models["main"].source;
    let before = model_scope_models(probed.db(), main1, sync1.project).clone();
    assert!(before.contains_key("renamed_macro"));
    assert!(before.contains_key("nested_macro"));

    let edited = nested_macro_project("renamed_macro", true, true);
    probed.reset();
    let state2 = sync_from_datamodel_incremental(probed.db_mut(), &edited, Some(&state1));
    let sync2 = state2.to_sync_result();
    let after = model_scope_models(probed.db(), sync2.models["main"].source, sync2.project);
    assert!(
        before == *after,
        "the reachable model handles must be unchanged"
    );
    assert_eq!(
        probed.counts().get("model_scope_models"),
        None,
        "the unchanged target set must backdate before the closure executes"
    );
    assert_eq!(
        probed.counts().get("model_module_targets"),
        Some(&(1, 1)),
        "only the lightweight target projection may scan the changed model"
    );
    assert_eq!(
        probed.counts().get("lowered_source_variable"),
        None,
        "module-scope discovery must not lower any equation"
    );
}

#[test]
fn removing_a_nested_macro_call_removes_its_transitive_module_scope() {
    let mut probed = ProbedDb::new();
    let initial = nested_macro_project("renamed_macro", true, false);
    let state1 = sync_from_datamodel_incremental(probed.db_mut(), &initial, None);
    let sync1 = state1.to_sync_result();
    let before = model_scope_models(probed.db(), sync1.models["main"].source, sync1.project);
    for expected in ["main", "renamed_macro", "nested_macro"] {
        assert!(
            before.contains_key(expected),
            "the initial scope must include {expected}"
        );
    }

    let without_call = nested_macro_project("renamed_macro", false, false);
    probed.reset();
    let state2 = sync_from_datamodel_incremental(probed.db_mut(), &without_call, Some(&state1));
    let sync2 = state2.to_sync_result();
    let after = model_scope_models(probed.db(), sync2.models["main"].source, sync2.project);
    assert_eq!(
        after.keys().map(Ident::as_str).collect::<Vec<_>>(),
        vec!["main"],
        "unreachable macro and renamed-module models must leave the unit scope"
    );
    assert_eq!(
        probed.counts().get("lowered_source_variable"),
        None,
        "topology removal must not lower any equation"
    );
}

#[test]
fn renaming_a_nested_macro_target_updates_only_the_lightweight_scope() {
    let mut probed = ProbedDb::new();
    let initial = nested_macro_project("original_macro", true, false);
    let state1 = sync_from_datamodel_incremental(probed.db_mut(), &initial, None);
    let sync1 = state1.to_sync_result();
    let before = model_scope_models(probed.db(), sync1.models["main"].source, sync1.project);
    assert!(before.contains_key("original_macro"));
    assert!(before.contains_key("nested_macro"));

    let renamed = nested_macro_project("renamed_macro", true, false);
    probed.reset();
    let state2 = sync_from_datamodel_incremental(probed.db_mut(), &renamed, Some(&state1));
    let sync2 = state2.to_sync_result();
    let after = model_scope_models(probed.db(), sync2.models["main"].source, sync2.project);
    assert!(after.contains_key("renamed_macro"));
    assert!(after.contains_key("nested_macro"));
    assert!(!after.contains_key("original_macro"));
    assert_eq!(
        probed.counts().get("lowered_source_variable"),
        None,
        "macro-target renaming must update topology without lowering equations"
    );
}

#[test]
fn a_two_hop_unit_conflict_uses_the_transitive_per_variable_views() {
    let project = x_project(
        sim_specs_with_units("month"),
        &[
            x_model(
                "main",
                vec![
                    x_aux("x", "1", Some("widget")),
                    x_module("sub_a", &[("x", "sub_a.input")], None),
                ],
            ),
            x_model(
                "sub_a",
                vec![
                    x_aux("input", "0", None),
                    x_module("sub_c", &[("input", "sub_c.input")], None),
                ],
            ),
            x_model("sub_c", vec![x_aux("input", "0", Some("gadget"))]),
        ],
    );
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    assert_eq!(
        model_scope_models(&db, sync.models["main"].source, sync.project)
            .keys()
            .map(Ident::as_str)
            .collect::<Vec<_>>(),
        ["main", "sub_a", "sub_c"]
    );

    let diagnostics = super::units::check_model_units::accumulated::<Diagnostic>(
        &db,
        sync.models["main"].source,
        sync.project,
    );
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == crate::common::ErrorCode::UnitMismatch
                && diagnostic.category.is_unit()
        }),
        "the widget/gadget conflict closes only through sub_a: {diagnostics:?}"
    );
}

#[test]
fn an_unknown_stdlib_prefixed_user_model_is_unit_checked() {
    let project = x_project(
        sim_specs_with_units("month"),
        &[
            x_model("main", vec![x_aux("x", "1", None)]),
            x_model(
                "stdlib\u{205a}unknown",
                vec![
                    x_aux("a", "1", Some("widget")),
                    x_aux("b", "2", Some("gadget")),
                    x_aux("c", "a + b", Some("widget")),
                ],
            ),
        ],
    );
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);

    let user_model = sync.models["stdlib\u{205a}unknown"].source;
    assert!(
        !source_model_is_stdlib(&db, user_model),
        "an unknown suffix remains a user model after production sync"
    );
    let diagnostics =
        super::units::check_model_units::accumulated::<Diagnostic>(&db, user_model, sync.project);
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == crate::common::ErrorCode::UnitMismatch
                && diagnostic.category.is_unit()
        }),
        "the user model's widget/gadget mismatch must reach check_model_units: {diagnostics:?}"
    );

    let real_stdlib = sync.models["stdlib\u{205a}smth1"].source;
    assert!(source_model_is_stdlib(&db, real_stdlib));
    assert!(
        super::units::check_model_units::accumulated::<Diagnostic>(&db, real_stdlib, sync.project,)
            .is_empty(),
        "a real generic stdlib template remains excluded from isolated unit checking"
    );
}

fn unrelated_models_project(other_equation: &str) -> crate::datamodel::Project {
    x_project(
        sim_specs_with_units("month"),
        &[
            x_model(
                "main",
                vec![
                    x_aux("driver", "5", Some("widget")),
                    x_aux("scaled", "driver * 2", Some("widget")),
                ],
            ),
            x_model(
                "other",
                vec![x_aux("value", other_equation, Some("gadget"))],
            ),
        ],
    )
}

#[test]
fn an_unrelated_models_edit_does_not_execute_main_unit_analysis_or_lowering() {
    let mut probed = ProbedDb::new();
    let initial = unrelated_models_project("1");
    let state1 = sync_from_datamodel_incremental(probed.db_mut(), &initial, None);
    let sync1 = state1.to_sync_result();
    super::units::check_model_units(probed.db(), sync1.models["main"].source, sync1.project);

    let edited = unrelated_models_project("2");
    probed.reset();
    let state2 = sync_from_datamodel_incremental(probed.db_mut(), &edited, Some(&state1));
    let sync2 = state2.to_sync_result();
    super::units::check_model_units(probed.db(), sync2.models["main"].source, sync2.project);
    assert_eq!(probed.counts().get("check_model_units"), None);
    assert_eq!(probed.counts().get("lowered_source_variable"), None);

    super::units::check_model_units(probed.db(), sync2.models["other"].source, sync2.project);
    assert_eq!(
        probed.counts().get("check_model_units"),
        Some(&(1, 1)),
        "the edited model's own unit analysis must execute"
    );
    assert_eq!(
        probed.counts().get("lowered_source_variable"),
        Some(&(1, 1)),
        "only the edited variable may be lowered"
    );
}

fn parent_child_project(child_equation: &str) -> crate::datamodel::Project {
    x_project(
        sim_specs_with_units("month"),
        &[
            x_model(
                "main",
                vec![
                    x_aux("driver", "5", Some("widget")),
                    x_module("sub", &[("driver", "sub.input")], None),
                    x_aux("combined", "sub.out", Some("widget")),
                ],
            ),
            x_model(
                "sub",
                vec![
                    x_aux("input", "0", Some("widget")),
                    x_aux("out", child_equation, Some("widget")),
                ],
            ),
        ],
    )
}

#[test]
fn a_module_targets_edit_rechecks_the_parent_but_lowers_only_the_edited_child() {
    let mut probed = ProbedDb::new();
    let initial = parent_child_project("input * 2");
    let state1 = sync_from_datamodel_incremental(probed.db_mut(), &initial, None);
    let sync1 = state1.to_sync_result();
    super::units::check_model_units(probed.db(), sync1.models["main"].source, sync1.project);

    let edited = parent_child_project("input * 3");
    probed.reset();
    let state2 = sync_from_datamodel_incremental(probed.db_mut(), &edited, Some(&state1));
    let sync2 = state2.to_sync_result();
    super::units::check_model_units(probed.db(), sync2.models["main"].source, sync2.project);
    assert_eq!(
        probed.counts().get("check_model_units"),
        Some(&(1, 1)),
        "the parent must re-run inference over its changed child"
    );
    assert_eq!(
        probed.counts().get("lowered_source_variable"),
        Some(&(1, 1)),
        "the child edit must reuse every other per-variable lowered memo"
    );
}
