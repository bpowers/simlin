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

/// One feedback loop, so the overlay has something to score.
fn feedback_loop() -> datamodel::Project {
    TestProject::new("overlay_keyed")
        .with_sim_time(0.0, 2.0, 1.0)
        .stock("population", "10", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "0.1", None)
        .build_datamodel()
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
    let mut probed = ProbedDb::new();
    let datamodel = feedback_loop();
    let project = sync_from_datamodel_incremental(probed.db_mut(), &datamodel, None).project;

    let round = |probed: &ProbedDb| {
        let with_overlay =
            assemble_simulation(probed.db(), project, "main".to_string(), LtmOverlay::On)
                .expect("the loop assembles under the overlay");
        let plain = assemble_simulation(probed.db(), project, "main".to_string(), LtmOverlay::Off)
            .expect("the loop assembles plain");
        collect_all_diagnostics(probed.db(), project, LtmOverlay::Off);
        collect_all_diagnostics(probed.db(), project, LtmOverlay::On);
        (with_overlay, plain)
    };

    let (first_on, first_off) = round(&probed);
    assert!(
        first_on.n_slots() > first_off.n_slots(),
        "the overlay's program carries the score slots the plain one does not"
    );

    probed.reset();
    let (again_on, again_off) = round(&probed);
    assert!(
        Arc::ptr_eq(&first_on, &again_on) && Arc::ptr_eq(&first_off, &again_off),
        "each overlay's assembly must be served from its own memo"
    );
    assert_eq!(
        re_executed(&probed),
        Vec::<String>::new(),
        "interleaving the two overlays must re-execute nothing: each is its own key"
    );
}

/// The overlay-independent derivations are shared: the LTM variable set is
/// derived once, whichever overlay first demanded it.
#[test]
fn the_ltm_derivation_is_derived_once_for_both_overlays() {
    let mut probed = ProbedDb::new();
    let datamodel = feedback_loop();
    let sync = sync_from_datamodel_incremental(probed.db_mut(), &datamodel, None);
    let (project, model) = (sync.project, sync.models["main"].source_model);

    assemble_simulation(probed.db(), project, "main".to_string(), LtmOverlay::Off)
        .expect("assembles plain");
    assert!(
        !probed.counts().contains_key("model_ltm_variables"),
        "a plain assembly must not derive the overlay"
    );

    probed.reset();
    let derived = model_ltm_variables(probed.db(), model, project);
    assert!(!derived.vars.is_empty(), "the loop is scored");
    assemble_simulation(probed.db(), project, "main".to_string(), LtmOverlay::On)
        .expect("assembles under the overlay");
    assert_eq!(
        probed
            .counts()
            .get("model_ltm_variables")
            .map(|(runs, _)| *runs),
        Some(1),
        "the derivation runs once and the overlay's assembly reads that memo"
    );
}
