// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The LTM overlay is an argument of the compile, not an input on the
//! project (`LtmOverlay`): both variants stay memoized side by side, so a
//! pass under one overlay never discards or re-verifies what a pass under the
//! other derived.

use std::sync::Arc;

use super::*;
use crate::db::exec_probe::ProbedDb;
use crate::test_common::TestProject;

/// The overlay keys two kinds of memo: a model's own layout, assembly and
/// diagnostics, and the shape a cross-module read resolves through
/// (`model_shape` of the sub-model, which carries its LTM section under
/// `On`). A module-free model reaches only the first kind, so every test
/// runs over both fixtures: one feedback loop, and the same loop routed
/// through a SMOOTH instance so `births` reads a module output.
fn fixtures() -> [(&'static str, datamodel::Project); 2] {
    let plain = TestProject::new("overlay_keyed")
        .with_sim_time(0.0, 2.0, 1.0)
        .stock("population", "10", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "0.1", None)
        .build_datamodel();
    let through_module = TestProject::new("overlay_keyed_module")
        .with_sim_time(0.0, 2.0, 1.0)
        .stock("population", "10", &["births"], &[], None)
        .aux("smoothed", "SMTH1(population, 3)", None)
        .flow("births", "smoothed * rate", None)
        .aux("rate", "0.1", None)
        .build_datamodel();
    [
        ("plain loop", plain),
        ("loop through a module", through_module),
    ]
}

/// How many distinct `model_shape` memos a first pass under both overlays
/// leaves behind. Only a module read resolves through a shape, so the plain
/// fixture leaves none and the module fixture leaves one per overlay for its
/// SMOOTH sub-model. This is the check that the module arm was reached, not
/// merely that nothing re-ran.
fn model_shape_keys(probed: &ProbedDb) -> usize {
    probed
        .counts()
        .get("model_shape")
        .map(|(_, keys)| *keys)
        .unwrap_or(0)
}

/// Every tracked query that ran since the probe's last reset, by name. The
/// one query excluded is `model_all_diagnostics`, whose body re-runs by
/// design on every revision (it reads an untracked value so the accumulator
/// replay stays reachable); it is a walk over memoized children, not work.
fn re_executed(probed: &ProbedDb) -> Vec<String> {
    probed
        .counts()
        .into_keys()
        .filter(|name| name != "model_all_diagnostics")
        .collect()
}

/// Assembling and collecting diagnostics under each overlay in turn leaves
/// the other overlay's memos intact: the second round of the same calls
/// re-executes no tracked query and hands back the same programs.
#[test]
fn both_overlays_stay_memoized_side_by_side() {
    for (fixture, datamodel) in fixtures() {
        let mut probed = ProbedDb::new();
        let project = sync_from_datamodel_incremental(probed.db_mut(), &datamodel, None).project;

        let round = |probed: &ProbedDb| {
            let with_overlay =
                assemble_simulation(probed.db(), project, "main".to_string(), LtmOverlay::On)
                    .unwrap_or_else(|e| panic!("{fixture}: assembles under the overlay: {e:?}"));
            let plain =
                assemble_simulation(probed.db(), project, "main".to_string(), LtmOverlay::Off)
                    .unwrap_or_else(|e| panic!("{fixture}: assembles plain: {e:?}"));
            collect_all_diagnostics(probed.db(), project, LtmOverlay::Off);
            collect_all_diagnostics(probed.db(), project, LtmOverlay::On);
            (with_overlay, plain)
        };

        let (first_on, first_off) = round(&probed);
        assert!(
            first_on.n_slots() > first_off.n_slots(),
            "{fixture}: the overlay's program carries the score slots the plain one does not"
        );
        let expected_shapes = if fixture == "plain loop" { 0 } else { 2 };
        assert_eq!(
            model_shape_keys(&probed),
            expected_shapes,
            "{fixture}: one shape memo per (sub-model, overlay) after the first round"
        );

        probed.reset();
        let (again_on, again_off) = round(&probed);
        assert!(
            Arc::ptr_eq(&first_on, &again_on) && Arc::ptr_eq(&first_off, &again_off),
            "{fixture}: each overlay's assembly must be served from its own memo"
        );
        assert_eq!(
            re_executed(&probed),
            Vec::<String>::new(),
            "{fixture}: interleaving the two overlays must re-execute nothing: each is its own key"
        );
    }
}

/// The overlay-independent derivations are shared: each model's LTM variable
/// set is derived once, whichever overlay first demanded it. The module
/// fixture has two models (main and the SMOOTH sub-model), so two keys.
#[test]
fn the_ltm_derivation_is_derived_once_for_both_overlays() {
    for (fixture, datamodel) in fixtures() {
        let mut probed = ProbedDb::new();
        let sync = sync_from_datamodel_incremental(probed.db_mut(), &datamodel, None);
        let (project, model) = (sync.project, sync.models["main"].source_model);

        assemble_simulation(probed.db(), project, "main".to_string(), LtmOverlay::Off)
            .unwrap_or_else(|e| panic!("{fixture}: assembles plain: {e:?}"));
        assert!(
            !probed.counts().contains_key("model_ltm_variables"),
            "{fixture}: a plain assembly must not derive the overlay"
        );

        probed.reset();
        let derived = model_ltm_variables(probed.db(), model, project);
        assert!(!derived.vars.is_empty(), "{fixture}: the loop is scored");
        assemble_simulation(probed.db(), project, "main".to_string(), LtmOverlay::On)
            .unwrap_or_else(|e| panic!("{fixture}: assembles under the overlay: {e:?}"));
        let models = if fixture == "plain loop" { 1 } else { 2 };
        assert_eq!(
            probed.counts().get("model_ltm_variables").copied(),
            Some((models, models)),
            "{fixture}: the derivation runs once per model and the overlay's assembly reads those memos"
        );
    }
}
