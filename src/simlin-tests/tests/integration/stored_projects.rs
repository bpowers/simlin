// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What a project stored as protobuf runs as.

use simlin_base::datamodel::Project;
use simlin_edit::apply_patch;
use simlin_engine::Vm;
use simlin_engine::db::{
    LtmOverlay, SimlinDb, compile_project_incremental, sync_from_datamodel_incremental,
};
use simlin_formats::buffa::Message;
use simlin_formats::{open_xmile, project_io, serde};

fn round_trip(project: &Project) -> Project {
    let bytes = serde::serialize(project).unwrap().encode_to_vec();
    serde::deserialize(project_io::Project::decode_from_slice(&bytes).unwrap()).unwrap()
}

/// A project whose specs run to 10 by steps of 1 and whose model overrides
/// them with its own: to 4 by steps of 0.5 (XMILE 1.0 section 4: a model "can
/// OPTIONALLY include `<sim_specs>` ... to override specific settings").
fn a_model_with_its_own_specs() -> Project {
    let xmile = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>specs</name><vendor>x</vendor><product version="1">x</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>10</stop><dt>1</dt></sim_specs>
  <model>
    <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>0.5</dt></sim_specs>
    <variables>
      <stock name="s"><eqn>0</eqn><inflow>f</inflow></stock>
      <flow name="f"><eqn>1</eqn></flow>
    </variables>
  </model>
</xmile>"#;
    open_xmile(&mut std::io::BufReader::new(xmile.as_bytes())).expect("the model opens")
}

/// The run of `project`'s root model: how many steps it saves, to what time,
/// by what step.
fn run_of(project: &Project) -> (usize, f64, f64) {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main", LtmOverlay::Off)
        .expect("the model compiles");
    let mut vm = Vm::new(compiled).expect("the model runs");
    vm.run_to_end().expect("the model runs");
    let results = vm.into_results();
    (results.step_count, results.specs.stop, results.specs.dt)
}

/// A model's own specs decide its run, so a stored project that kept only
/// the project's specs would run a different simulation.
#[test]
fn a_model_runs_under_its_own_specs_after_a_protobuf_trip() {
    let project = a_model_with_its_own_specs();
    assert_eq!(run_of(&project), (9, 4.0, 0.5), "the model's own specs");
    assert_eq!(run_of(&round_trip(&project)), (9, 4.0, 0.5));
}

/// Setting the simulation specs changes the run, whatever the stored project
/// holds: an editor shows and edits one set of specs, and the run a person
/// then sees has to be under them. A model can carry specs of its own, which
/// the run prefers (`db::assemble`); `SetSimSpecs` takes the root model's
/// away, so the project's are what it runs under.
#[test]
fn setting_the_sim_specs_of_a_stored_project_changes_its_run() {
    let mut stored = round_trip(&a_model_with_its_own_specs());
    let mut specs = stored.sim_specs.clone();
    specs.stop = 20.0;
    apply_patch(
        &mut stored,
        simlin_base::ProjectPatch {
            project_ops: vec![simlin_base::ProjectOperation::SetSimSpecs(specs)],
            models: vec![],
        },
    )
    .expect("the specs are set");
    let (_, stop, _) = run_of(&stored);
    assert!(
        stop == 20.0,
        "the run follows the specs that were set: it stops at {stop}"
    );
}
