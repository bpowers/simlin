// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::tools::test_support::{Host, inventory};

fn value(variable: &str, value: f64) -> ValueChange {
    ValueChange {
        variable: variable.to_string(),
        from_time: None,
        values: vec![(Ident::<Canonical>::new(variable), value)],
    }
}

fn equation(variable: &str, text: &str) -> EquationChange {
    EquationChange {
        variable: variable.to_string(),
        equation: text.to_string(),
    }
}

#[test]
fn a_plan_over_another_replaces_the_changes_of_the_variables_it_changes() {
    let base = RunPlan {
        values: vec![value("coverage", 8.0), value("adjustment_time", 4.0)],
        equations: vec![equation("orders", "12")],
        specs: SpecsChange {
            dt: Some(0.5),
            stop: Some(30.0),
            ..SpecsChange::default()
        },
        from: None,
    };
    let this = RunPlan {
        values: vec![value("coverage", 2.0)],
        // An equation change replaces a value change of the same variable,
        // and the other way round.
        equations: vec![equation("adjustment_time", "3")],
        specs: SpecsChange {
            dt: Some(0.25),
            ..SpecsChange::default()
        },
        from: Some("base".to_string()),
    };
    let over = this.over(&base);
    assert_eq!(over.values, vec![value("coverage", 2.0)]);
    assert_eq!(
        over.equations,
        vec![equation("orders", "12"), equation("adjustment_time", "3")]
    );
    assert_eq!(over.specs.dt, Some(0.25), "this run's specs win");
    assert_eq!(over.specs.stop, Some(30.0), "and the base's others stay");
    assert_eq!(
        over.from.as_deref(),
        Some("base"),
        "it was made over the base"
    );
    assert_eq!(RunPlan::default().over(&base), base);
}

#[test]
fn the_current_run_is_run_once_per_revision() {
    let mut host = Host::from_test_project(&inventory());
    let model = host.project.models[0].clone();
    let mut store = RunStore::default();
    let first = store.current(&mut host.workspace(), &model).unwrap();
    let again = store.current(&mut host.workspace(), &model).unwrap();
    assert!(Arc::ptr_eq(&first, &again));

    host.edit(|p| {
        p.models[0]
            .get_variable_mut("coverage")
            .unwrap()
            .set_scalar_equation("6")
    });
    let model = host.project.models[0].clone();
    let after = store.current(&mut host.workspace(), &model).unwrap();
    assert!(!Arc::ptr_eq(&first, &after));
    assert_eq!(after.revision, 1);
}

#[test]
fn a_staged_plan_leaves_the_hosts_database_as_it_found_it() {
    let mut host = Host::from_test_project(&inventory());
    let model = host.project.models[0].clone();
    let before = execute(&mut host.workspace(), &model, &RunPlan::default()).unwrap();
    let staged = RunPlan {
        equations: vec![equation("production", "10")],
        specs: SpecsChange {
            stop: Some(10.0),
            ..SpecsChange::default()
        },
        ..RunPlan::default()
    };
    let changed = execute(&mut host.workspace(), &model, &staged).unwrap();
    assert!(changed.step_count < before.step_count);
    let after = execute(&mut host.workspace(), &model, &RunPlan::default()).unwrap();
    assert_eq!(after.data, before.data, "the model runs exactly as before");
    assert_eq!(
        crate::db::collect_all_diagnostics(
            &host.db,
            host.db.current_source_project().unwrap(),
            LtmOverlay::Off
        ),
        vec![],
        "and its diagnostics are the model's"
    );
}

#[test]
fn a_finer_dt_drops_a_save_step_the_run_cannot_keep() {
    let project = crate::test_common::TestProject::new("saved")
        .with_sim_time(0.0, 4.0, 1.0)
        .with_save_step(1.0)
        .stock("s", "0", &["f"], &[], None)
        .flow("f", "1", None);
    let mut host = Host::from_test_project(&project);
    let model = host.project.models[0].clone();
    let coarse = RunPlan {
        specs: SpecsChange {
            dt: Some(2.0),
            ..SpecsChange::default()
        },
        ..RunPlan::default()
    };
    let results = execute(&mut host.workspace(), &model, &coarse).unwrap();
    assert_eq!(results.specs.dt, 2.0);
    assert_eq!(results.specs.save_step, 2.0);
}

#[test]
fn values_apply_at_their_times_in_order() {
    let project = crate::test_common::TestProject::new("steps")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("s", "0", &["f"], &[], None)
        .flow("f", "rate", None)
        .aux("rate", "1", None);
    let mut host = Host::from_test_project(&project);
    let model = host.project.models[0].clone();
    let at = |time: f64, v: f64| ValueChange {
        from_time: Some(time),
        ..value("rate", v)
    };
    // Given out of order: the later change is applied later.
    let plan = RunPlan {
        values: vec![at(6.0, 3.0), at(2.0, 2.0)],
        ..RunPlan::default()
    };
    let results = execute(&mut host.workspace(), &model, &plan).unwrap();
    let run = Run::new("steps".to_string(), 0, 0, plan, results);
    let offset = run.results.offsets[&Ident::<Canonical>::new("s")];
    let s = run.series(offset);
    // 1 a step to t = 2, 2 a step to t = 6, 3 a step after.
    assert_eq!(s[2], 2.0);
    assert_eq!(s[6], 2.0 + 4.0 * 2.0);
    assert_eq!(s[10], 10.0 + 4.0 * 3.0);
}

#[test]
fn a_value_at_the_start_time_reaches_initial_values_and_one_between_steps_takes_the_next() {
    let project = crate::test_common::TestProject::new("initial")
        .with_sim_time(0.0, 6.0, 1.0)
        .stock("s", "rate * 10", &["f"], &[], None)
        .flow("f", "rate", None)
        .aux("rate", "1", None);
    let mut host = Host::from_test_project(&project);
    let model = host.project.models[0].clone();
    let run = |host: &mut Host, from_time: Option<f64>, v: f64| {
        let plan = RunPlan {
            values: vec![ValueChange {
                from_time,
                ..value("rate", v)
            }],
            ..RunPlan::default()
        };
        let results = execute(&mut host.workspace(), &model, &plan).unwrap();
        let offset = results.offsets[&Ident::<Canonical>::new("s")];
        results.iter().map(|row| row[offset]).collect::<Vec<f64>>()
    };
    let from_start = run(&mut host, None, 5.0);
    assert_eq!(from_start[0], 50.0, "the initial value reads it");
    assert_eq!(run(&mut host, Some(0.0), 5.0), from_start);

    // Between the steps at 2 and 3, a change takes effect at 3.
    let between = run(&mut host, Some(2.5), 3.0);
    assert_eq!(&between[..4], &[10.0, 11.0, 12.0, 13.0]);
    assert_eq!(between[4], 16.0);
}

/// A value from a time holds from the first step at or after it, the step
/// `IF TIME >= t` turns on at: just after a step, mid-step, and on the grid.
#[test]
fn a_value_from_a_time_holds_from_the_first_step_at_or_after_it() {
    for (dt, time) in [
        (1.0, 2.2),
        (1.0, 2.5),
        (1.0, 2.9),
        (1.0, 3.0),
        (0.25, 5.1),
        (0.25, 5.0),
        (0.1, 0.3),
    ] {
        let project = crate::test_common::TestProject::new("steps")
            .with_sim_time(0.0, 8.0, dt)
            .stock("s", "0", &["f"], &[], None)
            .flow("f", "rate", None)
            .aux("rate", "1", None);
        let mut host = Host::from_test_project(&project);
        let model = host.project.models[0].clone();
        let plan = RunPlan {
            values: vec![ValueChange {
                from_time: Some(time),
                ..value("rate", 3.0)
            }],
            ..RunPlan::default()
        };
        let results = execute(&mut host.workspace(), &model, &plan).unwrap();
        let offset = results.offsets[&Ident::<Canonical>::new("s")];
        let changed: Vec<f64> = results.iter().map(|row| row[offset]).collect();

        // The same change written into the equation, as the engine steps it.
        let mut written = host.project.clone();
        written.models[0]
            .get_variable_mut("rate")
            .unwrap()
            .set_scalar_equation(&format!("IF TIME >= {time} THEN 3 ELSE 1"));
        let mut other = Host::new(written);
        let model = other.project.models[0].clone();
        let results = execute(&mut other.workspace(), &model, &RunPlan::default()).unwrap();
        let expected: Vec<f64> = results.iter().map(|row| row[offset]).collect();
        for (a, b) in changed.iter().zip(&expected) {
            assert!(
                (a - b).abs() < 1e-9,
                "dt {dt}, from {time}: {changed:?} != {expected:?}"
            );
        }
    }
}

#[test]
fn the_first_step_at_or_after_a_time() {
    assert_eq!(first_step_at_or_after(0.0, 1.0, 2.2), 3.0);
    assert_eq!(first_step_at_or_after(0.0, 1.0, 3.0), 3.0);
    assert_eq!(first_step_at_or_after(0.0, 0.25, 5.1), 5.25);
    // A time written as a step is that step, whatever the rounding.
    assert_eq!(first_step_at_or_after(0.0, 0.1, 0.3), 0.30000000000000004);
    assert_eq!(first_step_at_or_after(1.0, 0.5, 0.0), 1.0);
}

/// A run is fresh while the model has what it simulated: a diagram edit, a
/// sector, provenance change nothing, and an equation does.
#[test]
fn the_simulation_key_ignores_what_no_simulation_reads() {
    let project = crate::tools::test_support::with_diagram(inventory().build_datamodel());
    let key = simulation_key(&project);
    let mut moved = project.clone();
    crate::tools::test_support::zoom_the_diagram(&mut moved);
    assert_ne!(moved, project, "the diagram changed");
    moved.models[0].groups.push(datamodel::ModelGroup {
        name: "sector".to_string(),
        doc: None,
        parent: None,
        members: vec!["orders".to_string()],
        run_enabled: false,
    });
    if let Variable::Aux(aux) = moved.models[0].get_variable_mut("orders").unwrap() {
        aux.ai_state = Some(datamodel::AiState::C);
    }
    let coverage = moved.models[0].get_variable_mut("coverage").unwrap();
    coverage.set_documentation("weeks of orders the inventory aims to hold");
    coverage.set_units("week");
    assert_eq!(simulation_key(&moved), key);
    let mut edited = project.clone();
    edited.models[0]
        .get_variable_mut("coverage")
        .unwrap()
        .set_scalar_equation("5");
    assert_ne!(simulation_key(&edited), key);
    let mut respecified = project;
    respecified.sim_specs.stop = 30.0;
    assert_ne!(simulation_key(&respecified), key);
}

/// A save step that is not a multiple of DT leaves the results' last rows
/// unwritten at time zero: they are no part of the run.
#[test]
fn rows_past_the_last_saved_one_are_no_part_of_a_run() {
    let project = crate::test_common::TestProject::new("uneven")
        .with_sim_time(0.0, 10.0, 1.0 / 128.0)
        .stock("s", "0", &["f"], &[], None)
        .flow("f", "1", None);
    let mut datamodel = project.build_datamodel();
    datamodel.sim_specs.save_step = Some(datamodel::Dt::Dt(0.1));
    let mut host = Host::new(datamodel);
    let model = host.project.models[0].clone();
    let results = execute(&mut host.workspace(), &model, &RunPlan::default()).unwrap();
    let run = Run::new("uneven".to_string(), 0, 0, RunPlan::default(), results);
    let times = run.times();
    assert!(times.windows(2).all(|w| w[1] >= w[0]), "{times:?}");
    let last = *times.last().unwrap();
    assert!(
        last > 9.0,
        "the run ends near its stop, not at zero: {last}"
    );
    assert_eq!(run.row_at(last + 1.0), times.len() - 1);
    assert_eq!(run.row_at(0.05), 0);
}

/// A staged run restores the host's database however it ends, a panic
/// included, so the project's next compile is of the project.
#[test]
fn staging_restores_the_database_even_when_the_run_panics() {
    let mut host = Host::from_test_project(&inventory());
    let project = host.project.clone();
    let mut staged = project.clone();
    staged.models[0]
        .get_variable_mut("coverage")
        .unwrap()
        .set_scalar_equation("99");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _staging = Staging::new(&mut host.db, &project, &staged);
        panic!("a run that fails partway");
    }));
    assert!(result.is_err());
    let model = host.project.models[0].clone();
    let results = execute(&mut host.workspace(), &model, &RunPlan::default()).unwrap();
    let offset = results.offsets[&Ident::<Canonical>::new("coverage")];
    assert_eq!(
        results.iter().next().unwrap()[offset],
        4.0,
        "the project's own value"
    );
}

/// Specs from `start` to `stop` in steps of `dt`, saved every `save`.
fn run_specs(start: f64, stop: f64, dt: f64, save: f64) -> crate::results::Specs {
    crate::results::Specs::from(&datamodel::SimSpecs {
        start,
        stop,
        dt: datamodel::Dt::Dt(dt),
        save_step: Some(datamodel::Dt::Dt(save)),
        sim_method: datamodel::SimMethod::Euler,
        time_units: None,
    })
}

/// The number after `lead` in `reason`.
fn number_after(reason: &str, lead: &str) -> f64 {
    reason
        .split(lead)
        .nth(1)
        .and_then(|rest| rest.split([' ', ',']).next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("{lead:?} in {reason}"))
}

/// A run may hold and compute only so much, or what the model's own specs
/// cost when that is more; a refusal's numbers are the run's, and the specs
/// it suggests fit.
#[test]
fn a_run_costs_at_most_what_a_run_may_or_what_the_model_itself_does() {
    let own = run_specs(0.0, 20.0, 0.25, 0.25);
    assert_eq!(over_budget(&own, 12, &own), None);

    // Saving every step of a fine DT holds too much.
    let fine = run_specs(0.0, 20.0, 1e-4, 1e-4);
    let reason = over_budget(&fine, 12, &own).expect("refused");
    assert!(reason.contains("200001 rows of 12 values"), "{reason}");
    let dt = number_after(&reason, "a DT of at least ");
    assert_eq!(over_budget(&run_specs(0.0, 20.0, dt, dt), 12, &own), None);
    let stop = number_after(&reason, "a stop time of at most ");
    assert_eq!(
        over_budget(&run_specs(0.0, stop, 1e-4, 1e-4), 12, &own),
        None
    );

    // A coarse save step holds little, but a fine DT computes too much.
    let busy = run_specs(0.0, 1000.0, 1e-5, 1.0);
    let reason = over_budget(&busy, 12, &own).expect("refused");
    assert!(reason.contains("steps of 12 values"), "{reason}");
    let dt = number_after(&reason, "a DT of at least ");
    assert_eq!(
        over_budget(&run_specs(0.0, 1000.0, dt, 1.0), 12, &own),
        None
    );
    let stop = number_after(&reason, "a stop time of at most ");
    assert_eq!(
        over_budget(&run_specs(0.0, stop, 1e-5, 1.0), 12, &own),
        None
    );

    // A model whose own specs cost more runs as it stands, and no bigger.
    let large = run_specs(0.0, 100.0, 1e-4, 1e-4);
    assert_eq!(over_budget(&large, 12, &large), None);
    assert!(over_budget(&run_specs(0.0, 200.0, 1e-4, 1e-4), 12, &large).is_some());
}

/// The store simulates nothing while other work waits for the project: the
/// run of the model as it is waits, and a cached one is still answered.
#[test]
fn the_store_simulates_nothing_while_other_work_waits() {
    let mut host = Host::from_test_project(&inventory());
    let model = host.project.models[0].clone();
    let mut store = RunStore::default();
    let waiting = || true;
    let mut ws = Workspace {
        waiting: Some(&waiting),
        ..host.workspace()
    };
    let refusal = store.current(&mut ws, &model).err().expect("it stops");
    assert!(refusal.is_interrupted());
    assert!(store.current.is_none(), "nothing was run");

    let run = store.current(&mut host.workspace(), &model).unwrap();
    let mut ws = Workspace {
        waiting: Some(&waiting),
        ..host.workspace()
    };
    let cached = store.current(&mut ws, &model).unwrap();
    assert!(Arc::ptr_eq(&run, &cached));
}

/// A run taken a slice at a time is the run taken whole, and it stops
/// between two slices once other work waits for the project.
#[test]
fn a_run_in_slices_is_the_run_whole_and_stops_between_slices() {
    let mut host = Host::from_test_project(&inventory());
    let model = host.project.models[0].clone();
    let plan = RunPlan {
        values: vec![ValueChange {
            from_time: Some(7.3),
            ..value("coverage", 6.0)
        }],
        ..RunPlan::default()
    };
    let sliced = execute(&mut host.workspace(), &model, &plan).unwrap();
    let source_project = host.db.current_source_project().unwrap();
    let mut vm = crate::build_sim(
        &mut host.db,
        source_project,
        &host.project,
        "main",
        LtmOverlay::Off,
    )
    .ok()
    .unwrap();
    vm.run_to(7.5 - 0.125).ok().unwrap();
    vm.set_value(&Ident::<Canonical>::new("coverage"), 6.0)
        .ok()
        .unwrap();
    vm.run_to_end().ok().unwrap();
    let whole = vm.into_results();
    assert_eq!(sliced.data, whole.data, "bit for bit");

    let waiting = || true;
    let mut ws = Workspace {
        waiting: Some(&waiting),
        ..host.workspace()
    };
    assert!(matches!(
        execute(&mut ws, &model, &RunPlan::default()),
        Err(RunFailure::Stopped)
    ));
}
