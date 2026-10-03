// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A re-sync looks only at the variables that are not the ones the inputs
//! were last set from, and trusts that memory only while it is true of the
//! inputs.

use super::*;
use crate::db::exec_probe::ProbedDb;
use crate::patch::{ModelOperation, ModelPatch, ProjectPatch, apply_patch};
use crate::test_common::TestProject;

fn base() -> datamodel::Project {
    TestProject::new("identity")
        .aux("rate", "0.5", None)
        .aux("scale", "2", None)
        .flow("inflow", "rate * scale", None)
        .stock("level", "10", &["inflow"], &[], None)
        .aux("reader", "level * scale", None)
        .build_datamodel()
}

fn with_equation(project: &datamodel::Project, ident: &str, equation: &str) -> datamodel::Project {
    let Some(datamodel::Variable::Aux(aux)) = project.models[0].get_variable(ident).cloned() else {
        unreachable!("the fixture's {ident} is an aux")
    };
    let mut edited = project.clone();
    apply_patch(
        &mut edited,
        ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertAux(datamodel::Aux {
                    equation: datamodel::Equation::Scalar(equation.to_string()),
                    ..aux
                })],
            }],
        },
    )
    .expect("the edit applies");
    edited
}

/// How many variables `sync` extracted and compared, with its result.
fn compared<T>(sync: impl FnOnce() -> T) -> (usize, T) {
    VARIABLES_COMPARED.with(|count| count.set(0));
    let result = sync();
    (VARIABLES_COMPARED.with(std::cell::Cell::get), result)
}

fn equation(db: &SimlinDb, state: &PersistentSyncState, ident: &str) -> datamodel::Equation {
    state.models["main"].variables[ident]
        .source_var
        .equation(db)
        .clone()
}

fn scalar(text: &str) -> datamodel::Equation {
    datamodel::Equation::Scalar(text.to_string())
}

#[test]
fn a_resync_compares_only_the_variables_an_edit_replaced() {
    let project = base();
    let count = project.models[0].variables.len();
    let mut db = SimlinDb::default();
    let first = sync_from_datamodel_incremental(&mut db, &project, None);

    let (looked_at, second) =
        compared(|| sync_from_datamodel_incremental(&mut db, &project, Some(&first)));
    assert_eq!(looked_at, 0, "the project the inputs were set from");

    let edited = with_equation(&project, "scale", "3");
    let (looked_at, third) =
        compared(|| sync_from_datamodel_incremental(&mut db, &edited, Some(&second)));
    assert_eq!(looked_at, 1, "the one variable the edit replaced");
    assert_eq!(equation(&db, &third, "scale"), scalar("3"));
    assert_eq!(equation(&db, &third, "rate"), scalar("0.5"));

    // An equal project nothing shares is compared by value, every variable.
    let mut apart = edited.clone();
    apart.models[0].variables = apart.models[0].variables.to_vec().into();
    let (looked_at, _) =
        compared(|| sync_from_datamodel_incremental(&mut db, &apart, Some(&third)));
    assert_eq!(looked_at, count);
}

/// A rolled-back staging re-syncs the original project with the state from
/// BEFORE the staging, which remembers the original's own variables although
/// the inputs hold the staged ones. Trusting it would leave the staged
/// equation in place.
#[test]
fn a_rollback_restores_what_a_staged_sync_changed() {
    let project = base();
    let count = project.models[0].variables.len();
    let mut db = SimlinDb::default();
    db.sync(&project);

    let staged = with_equation(&project, "scale", "3");
    let (_, prev) = db.sync_staged(&staged);
    let (looked_at, ()) = compared(|| db.restore(&project, prev));
    assert_eq!(
        looked_at, count,
        "a state older than the inputs is not trusted"
    );
    let restored = db.sync_state.clone().expect("the db is synced");
    assert_eq!(equation(&db, &restored, "scale"), scalar("2"));

    // The restored state is the inputs' latest again.
    let (looked_at, _) = compared(|| db.sync(&project));
    assert_eq!(looked_at, 0);
    let (looked_at, _) = compared(|| db.sync(&staged));
    assert_eq!(looked_at, 1);
    let current = db.sync_state.clone().expect("the db is synced");
    assert_eq!(equation(&db, &current, "scale"), scalar("3"));
}

/// What a state remembers of an input is the variable the pass that made it
/// set the input from. An undo re-syncs a project that holds, for the
/// variable an edit replaced, the allocation the inputs were set from before
/// the edit: the state after the edit does not remember that one, so it is
/// compared and set back.
#[test]
fn an_undo_puts_back_the_variable_an_edit_replaced() {
    let project = base();
    let mut db = SimlinDb::default();
    db.sync(&project);
    let edited = with_equation(&project, "scale", "3");
    db.sync(&edited);

    let (looked_at, _) = compared(|| db.sync(&project));
    assert_eq!(looked_at, 1, "the one variable the undo puts back");
    let undone = db.sync_state.clone().expect("the db is synced");
    assert_eq!(equation(&db, &undone, "scale"), scalar("2"));

    let (looked_at, _) = compared(|| db.sync(&edited));
    assert_eq!(looked_at, 1, "and the one a redo replaces again");
    let redone = db.sync_state.clone().expect("the db is synced");
    assert_eq!(equation(&db, &redone, "scale"), scalar("3"));
}

/// A state used twice is the latest only the first time.
#[test]
fn a_state_reused_after_a_later_sync_is_not_trusted() {
    let project = base();
    let mut db = SimlinDb::default();
    let first = sync_from_datamodel_incremental(&mut db, &project, None);
    let edited = with_equation(&project, "scale", "3");
    let second = sync_from_datamodel_incremental(&mut db, &edited, Some(&first));
    assert_eq!(equation(&db, &second, "scale"), scalar("3"));

    let third = sync_from_datamodel_incremental(&mut db, &project, Some(&first));
    assert_eq!(equation(&db, &third, "scale"), scalar("2"));
}

/// Two variables of one canonical name share one input, which holds the
/// last's fields after every sync.
#[test]
fn a_repeated_name_keeps_the_last_variables_fields_across_syncs() {
    let mut project = base();
    let twin = |equation: &str| {
        datamodel::Variable::Aux(datamodel::Aux {
            ident: "twin".to_string(),
            equation: scalar(equation),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        })
    };
    project.models[0].variables.push(twin("1"));
    project.models[0].variables.push(twin("2"));
    let mut db = SimlinDb::default();
    let first = sync_from_datamodel_incremental(&mut db, &project, None);
    assert_eq!(equation(&db, &first, "twin"), scalar("2"));
    let second = sync_from_datamodel_incremental(&mut db, &project, Some(&first));
    assert_eq!(equation(&db, &second, "twin"), scalar("2"));
    let edited = with_equation(&project, "scale", "3");
    let third = sync_from_datamodel_incremental(&mut db, &edited, Some(&second));
    assert_eq!(equation(&db, &third, "twin"), scalar("2"));
}

/// A lookup table holding a NaN (an XMILE file can spell one in `<ypts>`)
/// equals itself, so a sync that compares it by value sets nothing and no
/// query runs again.
#[test]
fn a_resync_of_an_equal_project_holding_a_nan_reruns_nothing() {
    const XMILE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>nan</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>2</stop><dt>1</dt></sim_specs>
  <model><variables>
    <aux name="input"><eqn>TIME</eqn></aux>
    <aux name="looked up"><eqn>input</eqn>
      <gf><xscale min="0" max="2"/><ypts>0,NaN,1</ypts></gf></aux>
    <aux name="reader"><eqn>looked_up + 1</eqn></aux>
  </variables></model>
</xmile>"#;
    let open = || {
        crate::compat::open_xmile(&mut std::io::BufReader::new(XMILE.as_bytes()))
            .expect("the fixture opens")
    };
    let project = open();
    assert!(
        matches!(
            project.models[0].get_variable("looked_up"),
            Some(datamodel::Variable::Aux(aux))
                if aux.gf.as_ref().is_some_and(|gf| gf.y_points[1].is_nan())
        ),
        "the fixture's table holds a NaN"
    );

    let mut probed = ProbedDb::new();
    let overlay = crate::db::LtmOverlay::Off;
    let first = sync_from_datamodel_incremental(probed.db_mut(), &project, None);
    assemble_simulation(probed.db(), first.project, "main".to_string(), overlay)
        .expect("the fixture assembles");

    // The same file opened again: equal, and no allocation in common.
    let again = open();
    probed.reset();
    let second = sync_from_datamodel_incremental(probed.db_mut(), &again, Some(&first));
    assemble_simulation(probed.db(), second.project, "main".to_string(), overlay)
        .expect("the fixture assembles again");
    assert!(
        probed.counts().is_empty(),
        "re-syncing an equal project re-executed: {:?}",
        probed.counts()
    );
}
