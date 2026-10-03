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
        replacement: Replacement::Equation(text.to_string()),
    }
}

fn value_from(variable: &str, time: f64, v: f64) -> ValueChange {
    ValueChange {
        from_time: Some(time),
        ..value(variable, v)
    }
}

/// How this plan changes a variable, and how the plan it is made over did.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Change {
    /// A value from the start.
    Value,
    /// A value from time 5 on.
    ValueFrom5,
    /// A value from time 2 on.
    ValueFrom2,
    /// A replacement equation.
    Equation,
}

impl Change {
    const ALL: [Change; 4] = [
        Change::Value,
        Change::ValueFrom5,
        Change::ValueFrom2,
        Change::Equation,
    ];

    /// When the change takes effect.
    fn from(self) -> f64 {
        match self {
            Change::Value | Change::Equation => f64::NEG_INFINITY,
            Change::ValueFrom5 => 5.0,
            Change::ValueFrom2 => 2.0,
        }
    }

    /// A plan that makes this change to `x`, setting it to `v`.
    fn plan(self, v: f64) -> RunPlan {
        match self {
            Change::Value => RunPlan {
                values: vec![value("x", v)],
                ..RunPlan::default()
            },
            Change::ValueFrom5 => RunPlan {
                values: vec![value_from("x", 5.0, v)],
                ..RunPlan::default()
            },
            Change::ValueFrom2 => RunPlan {
                values: vec![value_from("x", 2.0, v)],
                ..RunPlan::default()
            },
            Change::Equation => RunPlan {
                equations: vec![equation("x", &v.to_string())],
                ..RunPlan::default()
            },
        }
    }
}

/// A variable's changes are a timeline: a change of this plan's replaces the
/// changes the base made to the variable from that time on, and keeps what the
/// base set before it. Every pairing of how the base changed a variable with
/// how this plan does; a variable this plan leaves alone keeps the base's.
#[test]
fn a_plan_over_another_keeps_what_the_base_set_before_its_own_changes() {
    for base_change in Change::ALL {
        for this_change in Change::ALL {
            let mut base = base_change.plan(1.0);
            base.values.push(value("untouched", 7.0));
            base.equations.push(equation("also_untouched", "8"));
            let this = this_change.plan(2.0);
            let over = this.over(&base);
            let what = format!("{this_change:?} over {base_change:?}");

            // The base's change survives exactly when it takes effect before
            // this plan's.
            let mut expected = RunPlan::default();
            expected.values.push(value("untouched", 7.0));
            expected.equations.push(equation("also_untouched", "8"));
            if base_change.from() < this_change.from() {
                let kept = base_change.plan(1.0);
                expected.values.splice(0..0, kept.values);
                expected.equations.splice(0..0, kept.equations);
            }
            expected.values.extend(this.values.clone());
            expected.equations.extend(this.equations.clone());
            assert_eq!(over.values, expected.values, "{what}");
            assert_eq!(over.equations, expected.equations, "{what}");
        }
    }

    let base = RunPlan {
        specs: SpecsChange {
            dt: Some(0.5),
            stop: Some(30.0),
            ..SpecsChange::default()
        },
        from: None,
        ..Change::Value.plan(1.0)
    };
    let this = RunPlan {
        specs: SpecsChange {
            dt: Some(0.25),
            ..SpecsChange::default()
        },
        from: Some("base".to_string()),
        ..RunPlan::default()
    };
    let over = this.over(&base);
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

/// A plan that changes only the run specs runs the model's own program under
/// them: nothing is staged, so no query runs during it, and the host's next
/// read of the project runs none either. The run is the one a model with
/// those specs makes, bit for bit. A plan that replaces an equation stages,
/// and so does a spec change on a model with a conveyor, whose expansion
/// reads the specs.
#[test]
fn a_plan_of_run_specs_alone_stages_nothing() {
    use crate::db::exec_probe::ProbedDb;
    let project = inventory().build_datamodel();
    let model = project.models[0].clone();
    let mut probed = ProbedDb::new();
    probed.db_mut().sync(&project);
    let run = |probed: &mut ProbedDb, plan: &RunPlan| {
        let mut ws = Workspace {
            project: &project,
            db: probed.db_mut(),
            revision: 0,
            waiting: None,
            cancelled: None,
        };
        execute(&mut ws, &model, plan).unwrap()
    };
    let bodies = |probed: &ProbedDb| -> usize { probed.counts().values().map(|(n, _)| n).sum() };
    run(&mut probed, &RunPlan::default());

    let specs = SpecsChange {
        stop: Some(33.0),
        dt: Some(0.125),
        method: Some(IntegrationMethod::Rk4),
        ..SpecsChange::default()
    };
    probed.reset();
    let results = run(
        &mut probed,
        &RunPlan {
            specs: specs.clone(),
            ..RunPlan::default()
        },
    );
    assert_eq!(bodies(&probed), 0, "{:?}", probed.counts());
    run(&mut probed, &RunPlan::default());
    assert_eq!(bodies(&probed), 0, "the host's next run compiles nothing");
    assert_eq!(
        (results.specs.stop, results.specs.dt, results.specs.method),
        (33.0, 0.125, crate::results::Method::RungeKutta4)
    );

    // The same specs written into the model.
    let mut written = project.clone();
    written.sim_specs.stop = 33.0;
    written.sim_specs.dt = datamodel::Dt::Dt(0.125);
    written.sim_specs.sim_method = datamodel::SimMethod::RungeKutta4;
    let mut other = Host::new(written);
    let other_model = other.project.models[0].clone();
    let expected = execute(&mut other.workspace(), &other_model, &RunPlan::default()).unwrap();
    let bits = |r: &Results| -> Vec<u64> {
        r.iter()
            .flat_map(|row| row.iter().map(|v| v.to_bits()))
            .collect()
    };
    assert_eq!(bits(&results), bits(&expected));

    probed.reset();
    run(
        &mut probed,
        &RunPlan {
            equations: vec![equation("production", "10")],
            ..RunPlan::default()
        },
    );
    assert!(
        bodies(&probed) > 0,
        "an equation is compiled in a staged copy"
    );

    let conveyor = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/conveyors/minimal_conveyor.xmile"
    ))
    .expect("the corpus has the conveyor model");
    let conveyor = crate::compat::open_xmile(&mut std::io::BufReader::new(conveyor.as_bytes()))
        .expect("the conveyor model opens");
    let mut host = Host::new(conveyor);
    let model = host.project.models[0].clone();
    let plan = RunPlan {
        specs: SpecsChange {
            stop: Some(6.0),
            ..SpecsChange::default()
        },
        ..RunPlan::default()
    };
    assert!(plan.stages(&host.project, &model.name));
    let shorter = execute(&mut host.workspace(), &model, &plan).unwrap();
    assert_eq!(shorter.specs.stop, 6.0);
    assert_eq!(shorter.step_count, 25);
}

/// A run saves a step at most once, so a plan whose DT is coarser than the
/// model's save step saves every step.
#[test]
fn a_coarser_dt_than_the_save_step_saves_every_step() {
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

/// A value from a time holds from the first step at or after it: just after
/// a step, mid-step, and on the grid, at DTs whose steps are not exact in
/// binary, under every integration method. Under Euler that is the step an
/// equation's `IF TIME >= t` turns on at, wherever the clock holds the step's
/// time exactly.
#[test]
fn a_value_from_a_time_holds_from_the_first_step_at_or_after_it() {
    use crate::datamodel::SimMethod;
    // (dt, the time, the index of the first step at or after it)
    let rows = [
        (1.0, 2.2, 3usize),
        (1.0, 2.5, 3),
        (1.0, 2.9, 3),
        (1.0, 3.0, 3),
        (0.25, 5.1, 21),
        (0.25, 5.0, 20),
        (0.1, 0.3, 3),
        (0.1, 1.1, 11),
        (0.1, 5.0, 50),
        (0.3, 0.9, 3),
        (0.3, 2.1, 7),
    ];
    for method in [
        SimMethod::Euler,
        SimMethod::RungeKutta2,
        SimMethod::RungeKutta4,
    ] {
        for (dt, time, first) in rows {
            let project = crate::test_common::TestProject::new("steps")
                .with_sim_time(0.0, 8.0, dt)
                .with_sim_method(method)
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
            let rate = results.offsets[&Ident::<Canonical>::new("rate")];
            let rates: Vec<f64> = results.iter().map(|row| row[rate]).collect();
            assert_eq!(
                rates.iter().position(|&r| r == 3.0),
                Some(first),
                "{method:?}, dt {dt}, from {time}"
            );
            // The same change written into the equation, as the engine steps
            // it under Euler, wherever the step's time is not below the time
            // it is written as (three DTs of 0.3 are below 0.9, and an
            // equation's own comparison then turns on a step later).
            if method != SimMethod::Euler || (first as f64 * dt) < time {
                continue;
            }
            let offset = results.offsets[&Ident::<Canonical>::new("s")];
            let changed: Vec<f64> = results.iter().map(|row| row[offset]).collect();
            let mut written = host.project.clone();
            written.models[0]
                .get_variable_mut("rate")
                .unwrap()
                .set_scalar_equation(&format!("IF TIME >= {time} THEN 3 ELSE 1"));
            let mut other = Host::new(written);
            let model = other.project.models[0].clone();
            let results = execute(&mut other.workspace(), &model, &RunPlan::default()).unwrap();
            let expected: Vec<f64> = results.iter().map(|row| row[offset]).collect();
            assert_eq!(changed, expected, "dt {dt}, from {time}");
        }
    }
}

/// A run is fresh while the model has what it simulated: the current run
/// survives a diagram edit and an edit of a variable's units, and is made
/// again after an edit of a model's own specs, which a model's run reads
/// though the project's are untouched.
#[test]
fn the_current_run_is_fresh_while_the_model_simulates_the_same() {
    let mut project = crate::tools::test_support::with_diagram(inventory().build_datamodel());
    project.models[0].sim_specs = Some(project.sim_specs.clone());
    let mut host = Host::new(project);
    let mut store = RunStore::default();
    let current = |host: &mut Host, store: &mut RunStore| {
        let model = host.project.models[0].clone();
        store.current(&mut host.workspace(), &model).unwrap()
    };
    let first = current(&mut host, &mut store);

    host.edit(crate::tools::test_support::zoom_the_diagram);
    host.edit(|p| {
        p.models[0]
            .get_variable_mut("coverage")
            .unwrap()
            .set_units("weeks")
    });
    assert!(Arc::ptr_eq(&first, &current(&mut host, &mut store)));

    host.edit(|p| p.models[0].sim_specs.as_mut().unwrap().stop = 40.0);
    let rerun = current(&mut host, &mut store);
    assert!(!Arc::ptr_eq(&first, &rerun));
    assert_eq!(rerun.results.specs.stop, 40.0);
    assert_eq!(*rerun.times().last().unwrap(), 40.0);
}

/// A run a tool reads is the rows the run saved, whatever the specs: with a
/// save step off the DT grid, or a stop time a fraction of a time unit on,
/// every row is a step the run evaluated, the last at the stop time, so a
/// constant flow reads as the constant it is.
#[test]
fn a_run_is_the_rows_it_saved_whatever_the_specs() {
    use crate::tools::Session;
    for (stop, dt, save_step) in [
        (10.0, 1.0 / 128.0, Some(0.1)),
        (10.0, 0.25, Some(0.3)),
        (10.0, 0.03, Some(0.1)),
        (10.0, 1.0, Some(2.5)),
        (0.7, 0.1, None),
    ] {
        let project = crate::test_common::TestProject::new("uneven")
            .with_sim_time(0.0, stop, dt)
            .stock("s", "0", &["f"], &[], None)
            .flow("f", "1", None);
        let mut datamodel = project.build_datamodel();
        datamodel.sim_specs.save_step = save_step.map(datamodel::Dt::Dt);
        let mut host = Host::new(datamodel);
        let answer = host.call(
            &mut Session::new("main"),
            "read_behavior",
            serde_json::json!({"variables": ["f", "s"]}),
        );
        let what = format!("stop {stop}, dt {dt}, save step {save_step:?}: {answer}");
        let (f, s) = (&answer["series"][0], &answer["series"][1]);
        assert_eq!(f["mode"]["kind"], "at_rest", "{what}");
        assert_eq!(f["end"], 1.0, "{what}");
        assert_eq!(s["mode"]["kind"], "linear", "{what}");
        // The last row is a step at or before the stop time, less than a
        // save step and a DT short of it.
        let every = save_step.unwrap_or(dt).max(dt);
        let end = s["max"]["time"].as_f64().unwrap();
        assert!(end <= stop + 1e-9 && end > stop - every - dt, "{what}");
        assert_eq!(s["end"], s["max"]["value"], "{what}");
    }

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
    assert_eq!(times.len(), 101);
    assert_eq!(times[100], 10.0);
    assert_eq!(run.row_at(11.0), 100);
    assert_eq!(run.row_at(0.05), 0);
}

/// A run at a finer DT saves the rows the model's own run does. The corpus's
/// pendulum saves every 0.1 at a DT of 1/128, 12.8 steps; at a quarter of
/// that DT a save is 51.2 steps, and both runs hold the same 1001 rows, the
/// last at the stop time -- from a battery check's plan, which names the save
/// step, and from an experiment's, which keeps the model's.
#[test]
fn a_finer_dt_saves_the_rows_the_models_own_run_does() {
    use crate::tools::Session;
    let pendulum = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/test-models/samples/pendulum/Single_Pendulum.mdl"
    ))
    .expect("the corpus has the pendulum model");
    let project = crate::compat::open_vensim(&pendulum).expect("the pendulum model opens");
    let dt = 1.0 / 128.0;
    assert_eq!(project.sim_specs.dt, datamodel::Dt::Dt(dt));
    assert_eq!(project.sim_specs.save_step, Some(datamodel::Dt::Dt(0.1)));
    let mut host = Host::new(project);
    let model = host.project.models[0].clone();

    let own = execute(&mut host.workspace(), &model, &RunPlan::default()).unwrap();
    let finer = RunPlan {
        specs: SpecsChange {
            dt: Some(dt / 4.0),
            save_step: Some(0.1),
            ..SpecsChange::default()
        },
        ..RunPlan::default()
    };
    let refined = execute(&mut host.workspace(), &model, &finer).unwrap();
    let times = |results: &Results| -> Vec<f64> { results.iter().map(|row| row[0]).collect() };
    let (own_times, refined_times) = (times(&own), times(&refined));
    assert_eq!((own_times.len(), refined_times.len()), (1001, 1001));
    assert_eq!((own_times[1000], refined_times[1000]), (100.0, 100.0));
    for (row, (own, refined)) in own_times.iter().zip(&refined_times).enumerate() {
        let save_time = row as f64 * 0.1;
        assert!(*own >= save_time - 1e-9 && *own < save_time + dt, "{own}");
        assert!(
            *refined >= save_time - 1e-9 && *refined < save_time + dt / 4.0,
            "{refined}"
        );
    }

    let mut session = Session::new("main");
    let answer = host.call(
        &mut session,
        "run_experiment",
        serde_json::json!({"name": "finer", "specs": {"dt": dt / 4.0}}),
    );
    assert_eq!(answer["specs"]["dt"], dt / 4.0, "{answer}");
    let Ok(kept) = session.run_results(host.workspace(), "finer") else {
        panic!("the experiment's run is kept");
    };
    assert_eq!(times(&kept.results), refined_times);
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

/// The bounds a refusal suggests print with the three digits they have, on
/// either side of one: down for a stop time a run may reach, up for a DT it
/// must be at least.
#[test]
fn a_suggested_bound_prints_with_three_digits() {
    for (x, down, up) in [
        (11_456_789.0, "11400000", "11500000"),
        (11_400_000.0, "11400000", "11400000"),
        (1234.5, "1230", "1240"),
        (100.0, "100", "100"),
        (7.0, "7", "7"),
        (0.012_345_6, "0.0123", "0.0124"),
        (0.3, "0.3", "0.3"),
    ] {
        assert_eq!(rounded_down(x).to_string(), down, "{x}");
        assert_eq!(rounded_up(x).to_string(), up, "{x}");
    }
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

    // A model whose own specs cost more runs as it stands, and no bigger:
    // one that saves more than a run may hold, and one that saves little and
    // computes more than a run may.
    let large = run_specs(0.0, 100.0, 1e-4, 1e-4);
    assert_eq!(over_budget(&large, 12, &large), None);
    assert!(over_budget(&run_specs(0.0, 200.0, 1e-4, 1e-4), 12, &large).is_some());
    let long = run_specs(0.0, 100.0, 1e-6, 1.0);
    assert!(
        long.final_step() as usize * 12 > MAX_RUN_STEPS && long.n_chunks * 12 < MAX_RUN_VALUES,
        "the row is about steps alone"
    );
    assert_eq!(over_budget(&long, 12, &long), None);
    let reason = over_budget(&run_specs(0.0, 200.0, 1e-6, 1.0), 12, &long).expect("refused");
    assert!(reason.contains("steps of 12 values"), "{reason}");

    // Its numbers are written as a person writes them, however far the
    // specs are from the model's: a count past counting is said so, and a
    // number of 63 digits has an exponent.
    let far = run_specs(0.0, 1e308, 1.0, 1.0);
    assert_eq!(far.n_chunks, usize::MAX);
    let reason = over_budget(&far, 12, &own).expect("refused");
    assert!(
        reason.contains("would save more rows than can be counted"),
        "{reason}"
    );
    assert!(reason.contains("a DT of at least 6.01e302"), "{reason}");
    let dt = number_after(&reason, "a DT of at least ");
    assert_eq!(over_budget(&run_specs(0.0, 1e308, dt, dt), 12, &own), None);

    // A count too large for a float to hold every digit of is written to
    // three: 4e18 rows, not 4.0000000000000005e18.
    let reason = over_budget(&run_specs(0.0, 1e18, 0.25, 0.25), 12, &own).expect("refused");
    assert!(
        reason.contains("would save 4e18 rows of 12 values, 4.8e19 numbers"),
        "{reason}"
    );

    // A run from 1e18 has no stop time three digits can write after its
    // start, so none is suggested: only specs the tool takes are. The DT
    // suggested fits.
    let late = run_specs(1e18, 1.0000000001e18, 1.0, 1.0);
    let reason = over_budget(&late, 12, &own).expect("refused");
    assert!(!reason.contains("stop time"), "{reason}");
    let dt = number_after(&reason, "a DT of at least ");
    assert_eq!(
        over_budget(&run_specs(1e18, 1.0000000001e18, dt, dt), 12, &own),
        None
    );
    // Saving less often than every step, a DT is still what it suggests:
    // a longer save step alone would leave the steps to compute.
    let late = run_specs(1e18, 1.0000000001e18, 1.0, 2.0);
    let reason = over_budget(&late, 12, &own).expect("refused");
    assert!(!reason.contains("stop time"), "{reason}");
    let dt = number_after(&reason, "a DT of at least ");
    assert_eq!(
        over_budget(&run_specs(1e18, 1.0000000001e18, dt, 2.0), 12, &own),
        None
    );
    assert!(
        reason.split(' ').all(|word| word.len() < 24),
        "no number is written out in full: {reason}"
    );
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
    vm.run_until(7.3).ok().unwrap();
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

/// A run is taken in at most `RUN_SLICES` slices, however late a time it is
/// asked to run until: no step past the run's last is counted, so a time far
/// past the stop is the end of the run.
#[test]
fn a_run_takes_at_most_its_slices_however_late_it_is_asked_to_run_until() {
    let mut host = Host::from_test_project(&inventory());
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
    let asked = std::sync::atomic::AtomicUsize::new(0);
    // Other work begins to wait once the run has been asked more often than
    // it has slices.
    let waiting = || asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > RUN_SLICES as usize;
    let specs = vm.specs().clone();
    let mut slices = Slices {
        at: 0.0,
        slice: (specs.final_step() as f64 / RUN_SLICES).max(1.0),
        waiting: Some(&waiting),
    };
    assert!(
        slices.run_until(&mut vm, 1e300).is_ok(),
        "it ends without being stopped"
    );
    assert!(asked.into_inner() <= RUN_SLICES as usize);
}

/// A batch of runs starts no run once other work waits for the project, and
/// answers that it stopped rather than with the runs it did.
#[test]
fn a_batch_starts_no_run_while_other_work_waits() {
    let mut host = Host::from_test_project(&inventory());
    let model = host.project.models[0].clone();
    let plans: Vec<RunPlan> = [5.0, 6.0, 7.0]
        .into_iter()
        .map(|v| RunPlan {
            values: vec![value("coverage", v)],
            ..RunPlan::default()
        })
        .collect();
    let summarized = std::sync::atomic::AtomicUsize::new(0);
    let summarize = |_: &RunPlan, _: Results| {
        summarized.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    };
    // Waiting begins once the batch has asked at its start.
    let waiting = Host::waiting_after(1);
    let mut ws = Workspace {
        waiting: Some(&waiting),
        ..host.workspace()
    };
    let stopped = execute_values(&mut ws, &model, &plans, summarize);
    assert!(stopped.err().is_some_and(|err| err.is_interrupted()));
    assert_eq!(summarized.into_inner(), 0, "no run started");

    let done = execute_values(&mut host.workspace(), &model, &plans, |_, _| ()).ok();
    assert_eq!(done.map(|runs| runs.len()), Some(3));
}

/// A named run with `rows` rows of one value each, for the store's bookkeeping.
fn sized_run(name: &str, rows: usize) -> Run {
    let specs = run_specs(0.0, rows as f64 - 1.0, 1.0, 1.0);
    let results = Results {
        offsets: Default::default(),
        data: vec![0.0; rows].into_boxed_slice(),
        step_size: 1,
        step_count: rows,
        specs,
        is_vensim: false,
    };
    Run::new(name.to_string(), 0, 0, RunPlan::default(), results)
}

/// Which named runs still hold their results, in the store's order.
fn held(store: &RunStore) -> Vec<&str> {
    store
        .named
        .iter()
        .filter(|(_, kept)| matches!(kept, Kept::Run(_)))
        .map(|(name, _)| name.as_str())
        .collect()
}

/// The store keeps results within its budget, to the byte: at the budget
/// nothing goes, a byte over it the oldest run's results go, and the run just
/// kept keeps its results however little the budget is.
#[test]
fn the_store_drops_the_oldest_results_past_its_budget_and_never_the_newest() {
    let bytes = 10 * std::mem::size_of::<f64>();
    for (budget, expected) in [
        (3 * bytes, vec!["a", "b", "c"]),
        (3 * bytes - 1, vec!["b", "c"]),
        (bytes, vec!["c"]),
        (1, vec!["c"]),
    ] {
        let mut store = RunStore {
            byte_budget: Some(budget),
            ..RunStore::default()
        };
        for name in ["a", "b", "c"] {
            store.keep(sized_run(name, 10));
        }
        assert_eq!(held(&store), expected, "a budget of {budget} bytes");
        assert_eq!(store.named.len(), 3, "a run without results keeps its plan");
    }
}

/// A run's loop analysis, once it has one, counts against the store's budget
/// with its results.
#[test]
fn a_runs_loop_analysis_counts_against_the_budget() {
    use crate::tools::Session;
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    for name in ["a", "b"] {
        host.call(
            &mut session,
            "run_experiment",
            serde_json::json!({"name": name, "set": [{"variable": "coverage", "value": 6}]}),
        );
    }
    let bytes = |session: &Session, name: &str| match &session.runs.named[name] {
        Kept::Run(run) => run.bytes(),
        Kept::Planned { .. } => 0,
    };
    let results_only = bytes(&session, "a");
    host.call(
        &mut session,
        "analyze_loops",
        serde_json::json!({"run": "a"}),
    );
    let with_analysis = bytes(&session, "a");
    assert!(with_analysis > results_only, "the analysis is counted");

    // A budget the three runs' results fit in, and the analysis does not.
    let current = session.runs.current.as_ref().map_or(0, |run| run.bytes());
    session.runs.byte_budget =
        Some(current + 3 * results_only + (with_analysis - results_only) / 2);
    host.call(
        &mut session,
        "run_experiment",
        serde_json::json!({"name": "c", "set": [{"variable": "coverage", "value": 7}]}),
    );
    assert_eq!(held(&session.runs), ["b", "c"]);
}
