// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The run on its step grid ([`super`]): which steps a run evaluates and
//! saves, what its clock reads, and where `run_to` and `run_until` stop.
//! Included via `#[path]` so `use super::*` resolves vm's private items.

use super::*;
use crate::datamodel;
use crate::test_common::TestProject;

/// A stock filled at one unit a time unit, so its value is the time elapsed,
/// with a constant an override can set.
fn counter() -> std::sync::Arc<CompiledSimulation> {
    TestProject::new("counter")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("rate", "1", None)
        .flow("inflow", "rate", None)
        .stock("s", "0", &["inflow"], &[], None)
        .compile_incremental()
        .expect("the counter compiles")
}

fn specs(start: f64, stop: f64, dt: f64, save_step: Option<f64>, method: Method) -> Specs {
    Specs::from(&datamodel::SimSpecs {
        start,
        stop,
        dt: datamodel::Dt::Dt(dt),
        save_step: save_step.map(datamodel::Dt::Dt),
        sim_method: match method {
            Method::Euler => datamodel::SimMethod::Euler,
            Method::RungeKutta2 => datamodel::SimMethod::RungeKutta2,
            Method::RungeKutta4 => datamodel::SimMethod::RungeKutta4,
        },
        time_units: None,
    })
}

const METHODS: [Method; 3] = [Method::Euler, Method::RungeKutta2, Method::RungeKutta4];

fn column(results: &Results, name: &str) -> Vec<f64> {
    let off = results.offsets[&Ident::new(name)];
    results.iter().map(|row| row[off]).collect()
}

/// Specs as a person types them -- a DT that is no power of two, a start in
/// the thousands, a stop time a fraction of a time unit on -- run to the stop
/// time: a row for every step, the last at the stop time, the stock there
/// what the whole horizon integrates to. And a second call once the run is
/// over evaluates nothing.
#[test]
fn a_run_ends_at_the_stop_time_for_specs_as_typed() {
    let sim = counter();
    let mut checked = 0;
    for start in [0.0, 1.0, 1900.0, 2000.0] {
        for dt in [1.0, 0.25, 0.1, 0.2, 0.3, 0.05, 0.01, 0.7, 1.0 / 3.0] {
            for steps in [3u32, 7, 13, 21, 100] {
                // The stop time as typed: to nine places.
                let stop = ((start + f64::from(steps) * dt) * 1e9).round() / 1e9;
                let mut vm =
                    Vm::with_specs(sim.clone(), specs(start, stop, dt, None, Method::Euler))
                        .unwrap();
                vm.run_to_end().unwrap();
                vm.run_to_end().unwrap();
                vm.run_to(stop + 5.0 * dt).unwrap();
                let results = vm.into_results();
                let what = format!("{start}..{stop} by {dt}");
                assert_eq!(results.step_count, steps as usize + 1, "{what}");
                let time = column(&results, "time");
                let s = column(&results, "s");
                let last = steps as usize;
                assert!((time[last] - stop).abs() <= 1e-9, "{what}: {}", time[last]);
                assert!(
                    (s[last] - f64::from(steps) * dt).abs() <= 1e-9 * (1.0 + s[last].abs()),
                    "{what}: {}",
                    s[last]
                );
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 180);
}

/// The clock is counted: every saved time is `start + k * dt` to the bit,
/// under every integration method, so the step a person calls 5 is at 5.
#[test]
fn the_clock_reads_the_time_each_step_names() {
    let sim = counter();
    for method in METHODS {
        let run = specs(0.0, 100.0, 0.1, None, method);
        let mut vm = Vm::with_specs(sim.clone(), run.clone()).unwrap();
        vm.run_to_end().unwrap();
        let time = column(&vm.into_results(), "time");
        assert_eq!(time.len(), 1001);
        for (k, t) in time.iter().enumerate() {
            assert_eq!(t.to_bits(), run.time_at(k as u64).to_bits(), "step {k}");
        }
        assert_eq!(time[50], 5.0);
        assert_eq!(time[1000], 100.0);
    }
}

/// The rows a run saves are the rows the specs count: each the first step at
/// or after its save time, for a save step on the DT grid, off it, and
/// shorter than a DT; and `step_count` is how many were saved, whether the
/// run finished or stopped part-way.
#[test]
fn a_run_saves_the_first_step_at_or_after_each_save_time() {
    let sim = counter();
    for method in METHODS {
        for (dt, save_step, expected) in [
            (1.0, 4.0, vec![0.0, 4.0, 8.0]),
            (1.0, 2.5, vec![0.0, 3.0, 5.0, 8.0, 10.0]),
            (0.25, 0.3, {
                // 34 save times; each row the first quarter at or after it.
                (0..34)
                    .map(|m| (f64::from(m) * 0.3 / 0.25 - 1e-6).ceil() * 0.25)
                    .collect()
            }),
            (2.0, 0.5, vec![0.0, 2.0, 4.0, 6.0, 8.0, 10.0]),
        ] {
            let run = specs(0.0, 10.0, dt, Some(save_step), method);
            let mut vm = Vm::with_specs(sim.clone(), run.clone()).unwrap();
            vm.run_to_end().unwrap();
            let results = vm.into_results();
            let what = format!("{method:?} dt {dt} save {save_step}");
            assert_eq!(results.step_count, run.n_chunks, "{what}");
            let time = column(&results, "time");
            assert_eq!(time, expected, "{what}");
            // Every saved row is a step the run evaluated: the stock is the
            // time elapsed and the flow is its constant.
            assert_eq!(column(&results, "s"), expected, "{what}");
            assert!(
                column(&results, "inflow").iter().all(|&f| f == 1.0),
                "{what}"
            );
        }
    }

    let mut vm = Vm::with_specs(sim, specs(0.0, 10.0, 0.5, Some(2.0), Method::Euler)).unwrap();
    vm.run_to(5.0).unwrap();
    let partial = vm.into_results();
    assert_eq!(partial.step_count, 3, "rows at 0, 2 and 4");
    assert_eq!(column(&partial, "time"), [0.0, 2.0, 4.0]);
}

/// `run_to(t)` evaluates the step at `t`, so a value set after it holds from
/// the next step; `run_until(t)` stops before that step, so a value set after
/// it holds from the step at or after `t`. A time written as a step's is that
/// step, and one between two steps is the step after.
#[test]
fn run_until_stops_before_the_step_run_to_evaluates() {
    let sim = counter();
    // (dt, time, the index of the first step at or after it)
    let rows = [
        (1.0, 2.2, 3usize),
        (1.0, 3.0, 3),
        (0.25, 5.1, 21),
        (0.25, 5.0, 20),
        (0.1, 0.3, 3),
        (0.1, 1.1, 11),
        (0.1, 5.0, 50),
        (0.3, 0.9, 3),
        (0.3, 2.1, 7),
        (0.2, 9.6, 48),
        (1.0, 0.0, 0),
        (1.0, -4.0, 0),
    ];
    for method in METHODS {
        for (dt, time, first) in rows {
            let run = specs(0.0, 10.0, dt, None, method);
            let rate = Ident::new("rate");

            let mut vm = Vm::with_specs(sim.clone(), run.clone()).unwrap();
            vm.run_until(time).unwrap();
            vm.set_value(&rate, 3.0).unwrap();
            vm.run_to_end().unwrap();
            let changed = column(&vm.into_results(), "rate");
            let what = format!("{method:?} dt {dt} until {time}");
            assert_eq!(
                changed.iter().position(|&r| r == 3.0),
                Some(first),
                "{what}"
            );

            let mut vm = Vm::with_specs(sim.clone(), run.clone()).unwrap();
            vm.run_to(time).unwrap();
            vm.set_value(&rate, 3.0).unwrap();
            vm.run_to_end().unwrap();
            let changed = column(&vm.into_results(), "rate");
            // The step at or before the time is evaluated; before the start
            // there is none.
            let after = run.step_at_or_before(time) + 1.0;
            assert_eq!(
                changed.iter().position(|&r| r == 3.0),
                Some(after.max(0.0) as usize),
                "{what}: run_to"
            );
        }
    }
}

/// Between two calls the clock stands at a step that is not yet evaluated:
/// its time is the step's, its stocks are final, and its flows are a preview
/// of the values as they are.
#[test]
fn a_stopped_run_rests_at_an_unevaluated_step() {
    let sim = counter();
    let mut vm = Vm::with_specs(sim, specs(0.0, 10.0, 0.5, None, Method::Euler)).unwrap();
    vm.run_until(4.0).unwrap();
    let at = |vm: &Vm, name: &str| vm.get_value_now(vm.get_offset(&Ident::new(name)).unwrap());
    assert_eq!(at(&vm, "time"), 4.0);
    assert_eq!(at(&vm, "s"), 4.0);
    assert_eq!(at(&vm, "inflow"), 1.0);
    assert_eq!(
        vm.get_series(&Ident::new("s")).unwrap().len(),
        8,
        "rows 0..3.5"
    );
}

/// A run rests at the step after the last it evaluated, unevaluated, and a
/// finished run one step past the stop: its stocks integrated through the
/// final step, its flows a preview. So the clock never goes back from one
/// call to the next, whatever the save step, and a host that steps to its own
/// clock until it passes the stop finishes. Once it is over a run takes no
/// step and leaves its resting row alone, a value set on it since included.
#[test]
fn a_finished_run_rests_one_step_past_the_stop() {
    let at = |vm: &Vm, name: &str| vm.get_value_now(vm.get_offset(&Ident::new(name)).unwrap());
    // (save step, the time of the last saved row)
    for (save_step, last) in [(None, 10.0), (Some(4.0), 8.0), (Some(2.5), 10.0)] {
        for method in METHODS {
            let what = format!("{method:?}, save {save_step:?}");
            let fresh = || {
                Vm::with_specs(counter(), specs(0.0, 10.0, 1.0, save_step, method)).expect("a run")
            };
            let mut vm = fresh();
            let mut seen = Vec::new();
            for target in [3.0, 8.0, 9.0, 10.0, 12.0] {
                vm.run_to(target).unwrap();
                seen.push(at(&vm, "time"));
            }
            assert_eq!(seen, [4.0, 9.0, 10.0, 11.0, 11.0], "{what}");
            assert_eq!(
                at(&vm, "s"),
                11.0,
                "{what}: integrated through the final step"
            );
            assert_eq!(at(&vm, "inflow"), 1.0, "{what}: a preview");

            // A host stepping to its own clock until it passes the stop.
            let mut host = fresh();
            host.run_initials().unwrap();
            let mut calls = 0;
            while at(&host, "time") <= 10.0 && calls < 100 {
                let time = at(&host, "time");
                host.run_to(time).unwrap();
                calls += 1;
            }
            assert_eq!(calls, 11, "{what}: one call a step");

            vm.set_value(&Ident::new("rate"), 5.0).unwrap();
            assert_eq!(at(&vm, "rate"), 5.0);
            vm.run_to(25.0).unwrap();
            vm.run_to_end().unwrap();
            assert_eq!(
                at(&vm, "rate"),
                5.0,
                "{what}: a finished run is left as it is"
            );
            assert_eq!(at(&vm, "inflow"), 1.0, "{what}: and not previewed again");
            assert_eq!(at(&vm, "time"), 11.0, "{what}");
            let results = vm.into_results();
            let rows = column(&results, "s");
            assert_eq!(
                rows[rows.len() - 1],
                last,
                "{what}: its results end at the last saved row"
            );
        }
    }
}

/// A run too long to hold its rows is refused with its numbers, not
/// allocated, wrapped or left to abort the process: one whose rows cannot be
/// counted (a DT of 1e-320), and one whose rows can be counted and are more
/// than any allocator gives (1e14 rows: petabytes). The numbers are written
/// as a person writes them, not as 320 digits.
#[test]
fn a_run_with_more_rows_than_can_be_held_is_refused() {
    for (dt, reason) in [
        (
            1e-320,
            "a run from 0 to 1 in steps of 1e-320, saving every 1e-320, saves more rows than \
             can be counted, each of 7 values: more than can be held in memory",
        ),
        (
            1e-14,
            "a run from 0 to 1 in steps of 1e-14, saving every 1e-14, saves 100000000000001 \
             rows, each of 7 values: more than can be held in memory",
        ),
    ] {
        let Err(err) = Vm::with_specs(counter(), specs(0.0, 1.0, dt, None, Method::Euler)) else {
            panic!("a run of {} rows is refused", 1.0 / dt);
        };
        assert_eq!(err.code, ErrorCode::BadSimSpecs, "dt {dt}");
        assert_eq!(err.to_string(), reason, "dt {dt}");
    }
}

/// The specs a VM is given are the ones it checks: a program compiled under
/// specs a run can be made with is not run under specs one cannot.
#[test]
fn a_vm_refuses_run_specs_no_run_can_be_made_with() {
    let sim = counter();
    assert!(Vm::with_specs(sim.clone(), specs(0.0, 10.0, 1.0, None, Method::Euler)).is_ok());
    for (start, stop, dt, reason) in [
        (10.0, 0.0, 1.0, "end time has to be after start time"),
        (0.0, 10.0, 0.0, "dt must be greater than 0"),
        (0.0, 10.0, -1.0, "dt must be greater than 0"),
        (0.0, 10.0, f64::NAN, "dt must be greater than 0"),
    ] {
        let Err(err) = Vm::with_specs(sim.clone(), specs(start, stop, dt, None, Method::Euler))
        else {
            panic!("{start}..{stop} by {dt} is refused");
        };
        assert_eq!(err.code, ErrorCode::BadSimSpecs);
        assert_eq!(err.to_string(), reason);
    }
}

/// Whatever finite numbers the specs hold, a run is either refused with a
/// reason or is a run whose first row is step zero at the start time: no
/// spec makes a run of no rows, and none panics. The sweep includes specs
/// whose intermediate quantities overflow: a save step 1e308 times DT, a
/// horizon of 1e308, a DT of 1e-320.
#[test]
fn every_finite_spec_is_a_run_from_step_zero_or_a_refusal() {
    let sim = counter();
    let time = Ident::new("time");
    let (mut ran, mut refused) = (0, 0);
    for start in [0.0, 1900.0, -1e308, 1e308, 1e-300] {
        for span in [0.0, 1.0, 7.3, 1e-300, 1e308] {
            let stop: f64 = start + span;
            if !stop.is_finite() {
                continue;
            }
            // Each DT makes a run of a few steps or one of more rows than
            // can be held: nothing between, which would be a long test.
            for dt in [1e-320, 1e-14, 0.25, 1.0, 1e306, f64::MAX] {
                for save_step in [None, Some(1e-320), Some(0.1), Some(1e308), Some(f64::MAX)] {
                    // The methods take turns: the grid is the same under each.
                    for method in [METHODS[(ran + refused) % METHODS.len()]] {
                        let specs = specs(start, stop, dt, save_step, method);
                        let what = format!("{start}..{stop} by {dt}, save {save_step:?}");
                        let every = specs.save_step_in_steps();
                        assert!(every.is_finite() && every >= 1.0, "{what}: {every}");
                        assert_eq!(specs.saved_row_step(0), 0.0, "{what}");
                        assert!(specs.n_chunks >= 1, "{what}");
                        let last = specs.final_step();
                        let mut vm = match Vm::with_specs(sim.clone(), specs) {
                            Ok(vm) => vm,
                            Err(err) => {
                                assert_eq!(err.code, ErrorCode::BadSimSpecs, "{what}");
                                assert!(!err.to_string().is_empty(), "{what}");
                                refused += 1;
                                continue;
                            }
                        };
                        ran += 1;
                        // The first step, and the whole run where it is short.
                        if last <= 1000 {
                            vm.run_to_end().expect("the run");
                        } else {
                            vm.run_to(start).expect("the first step");
                        }
                        let times = vm.get_series(&time).expect("time is a series");
                        assert_eq!(times[0], start, "{what}");
                        if last <= 1000 {
                            assert_eq!(times.len(), vm.specs().n_chunks, "{what}");
                        }
                    }
                }
            }
        }
    }
    assert!(ran >= 100 && refused >= 50, "{ran} ran, {refused} refused");
}

/// PULSE fires at the first step at or after its time, read on the step
/// grid: a pulse time written as a step's is that step though the step's own
/// time is a unit in the last place below it, a pulse time between two steps
/// is the later of them, and one after the last step never fires. It carries
/// `volume / dt` for that one step, again every interval.
#[test]
fn a_pulse_fires_at_the_first_step_at_or_after_its_time() {
    let fired = |results: &HashMap<String, Vec<f64>>| -> Vec<usize> {
        results["p"]
            .iter()
            .enumerate()
            .filter(|(_, v)| **v != 0.0)
            .map(|(i, _)| i)
            .collect()
    };
    // (dt, the pulse's time, the step it fires at). The second row is one a
    // raw comparison of the clock with the pulse time gets wrong: three steps
    // of 0.3 are below 0.9.
    let rows = [
        (0.1, 5.0, Some(50usize)),
        (0.3, 0.9, Some(3)),
        (1.0, 3.0, Some(3)),
        (1.0, 3.2, Some(4)),
        (1.0, 3.7, Some(4)),
        (1.0, 3.999, Some(4)),
        (1.0, 10.0, Some(10)),
        (1.0, 10.4, None),
    ];
    let (dt, at, step) = rows[1];
    assert!(step.is_some_and(|step| dt * (step as f64) < at));
    for (dt, at, step) in rows {
        let results = TestProject::new("pulse")
            .with_sim_time(0.0, 10.0, dt)
            .aux("p", &format!("PULSE(2, {at})"), None)
            .run_vm_expecting_success();
        assert_eq!(
            fired(&results),
            step.into_iter().collect::<Vec<_>>(),
            "dt {dt}, PULSE(2, {at})"
        );
        if let Some(step) = step {
            assert!((results["p"][step] - 2.0 / dt).abs() < 1e-9);
        }
    }

    let results = TestProject::new("train")
        .with_sim_time(0.0, 2.0, 0.1)
        .aux("p", "PULSE(1, 0.3, 0.7)", None)
        .run_vm_expecting_success();
    assert_eq!(fired(&results), [3, 10, 17]);
}

/// A declared save step that is not a whole number of time steps is reported
/// with what the saved rows are instead: once for the project's specs, and
/// under its model for a model's own. A save step on the step grid, or none,
/// is not.
#[test]
fn a_save_step_off_the_step_grid_is_reported() {
    use crate::db::{Diagnostic, DiagnosticError, DiagnosticSeverity, LtmOverlay, SimlinDb};
    let diagnostics = |project: &datamodel::Project| -> Vec<Diagnostic> {
        let mut db = SimlinDb::default();
        let source = db.sync(project);
        crate::db::collect_all_diagnostics(&db, source, LtmOverlay::Off)
            .into_iter()
            .filter(|d| {
                d.severity == DiagnosticSeverity::Warning
                    && d.code() == ErrorCode::SaveStepOffTheStepGrid
            })
            .collect()
    };
    let warnings = |project: &datamodel::Project| -> Vec<(String, String)> {
        diagnostics(project)
            .into_iter()
            .map(|d| match &d.error {
                DiagnosticError::Model(err) => {
                    (d.model.clone(), err.get_details().unwrap_or_default())
                }
                _ => unreachable!("the warning is a model error"),
            })
            .collect()
    };
    let with = |dt: f64, save_step: Option<f64>| {
        let mut project = TestProject::new("saved")
            .with_sim_time(0.0, 10.0, dt)
            .aux("x", "1", None)
            .build_datamodel();
        project.sim_specs.save_step = save_step.map(datamodel::Dt::Dt);
        project
    };

    for (dt, save_step) in [
        (0.25, None),
        (0.25, Some(1.0)),
        (0.1, Some(0.3)),
        (0.5, Some(0.5)),
    ] {
        assert_eq!(warnings(&with(dt, save_step)), [], "{dt}, {save_step:?}");
    }

    let off = warnings(&with(0.0078125, Some(0.1)));
    assert_eq!(off.len(), 1, "{off:?}");
    assert_eq!(off[0].0, "", "the project's specs belong to no model");
    assert!(
        off[0].1.contains("0.1") && off[0].1.contains("first step at or after"),
        "{off:?}"
    );

    let shorter = warnings(&with(1.0, Some(0.5)));
    assert_eq!(shorter.len(), 1, "{shorter:?}");
    assert!(shorter[0].1.contains("every step is saved"), "{shorter:?}");
    // It reads as a fact about the project, which its specs are.
    let presented = crate::errors::format_diagnostic(&diagnostics(&with(1.0, Some(0.5)))[0]);
    assert_eq!(
        presented.message.as_deref(),
        Some(
            "warning in the project: save_step_off_the_step_grid -- the save step 0.5 is not a \
             whole number of time steps of 1: every step is saved"
        )
    );

    // A DT no run can be made with has no step grid to be off: that is the
    // specs' own error, and this warning says nothing beside it.
    for dt in [0.0, -1.0, f64::INFINITY, f64::NAN] {
        assert_eq!(warnings(&with(dt, Some(0.5))), [], "a DT of {dt}");
    }
    assert_eq!(warnings(&with(1.0, Some(f64::INFINITY))), []);

    // A model's own specs are reported under the model.
    let mut own = with(1.0, None);
    let mut specs = own.sim_specs.clone();
    specs.save_step = Some(datamodel::Dt::Dt(2.5));
    own.models[0].sim_specs = Some(specs);
    let reported = warnings(&own);
    assert_eq!(reported.len(), 1, "{reported:?}");
    assert_eq!(reported[0].0, "main");
}
