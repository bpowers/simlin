// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::test_common::TestProject;

fn replay(project: &TestProject, reducers: Reducers) -> Replay {
    let sim = project.compile_incremental().expect("the fixture compiles");
    Replay::new(&sim, reducers).expect("the program is cut")
}

fn slot(replay: &Replay, name: &str) -> usize {
    (0..replay.width)
        .find(|&slot| replay.name(slot) == Some(name))
        .unwrap_or_else(|| panic!("no slot {name}"))
}

fn edge(replay: &Replay, from: &str, to: &str, lag: Lag) -> usize {
    let (from, to) = (slot(replay, from), slot(replay, to));
    replay
        .edges()
        .iter()
        .position(|e| (e.from, e.to, e.lag) == (from, to, lag))
        .unwrap_or_else(|| panic!("no edge from slot {from} to slot {to}"))
}

/// An edge's score at each step of the run.
fn score_series(replay: &mut Replay, edge: usize) -> Vec<f64> {
    let steps: Vec<usize> = (0..replay.n_steps()).collect();
    let scores = replay.scores(&steps);
    scores[edge * steps.len()..(edge + 1) * steps.len()].to_vec()
}

#[track_caller]
fn assert_series(got: &[f64], want: &[f64]) {
    assert_eq!(got.len(), want.len());
    for (step, (g, w)) in got.iter().zip(want).enumerate() {
        assert!(
            (g - w).abs() <= 1e-12 * g.abs().max(w.abs()).max(1.0),
            "step {step}: {g} is not {w} (of {got:?}, {want:?})"
        );
    }
}

#[test]
fn a_score_is_the_change_the_source_alone_makes_over_the_whole_change() {
    let project = TestProject::new("product")
        .with_sim_time(0.0, 6.0, 1.0)
        .aux("x", "TIME * TIME + 1", None)
        .aux("w", "20 - 3 * TIME", None)
        .aux("z", "x * w", None);
    let mut replay = replay(&project, Reducers::Inline);
    let (x, w) = (|t: f64| t * t + 1.0, |t: f64| 20.0 - 3.0 * t);
    let z = |t: f64| x(t) * w(t);
    let want = |partial: &dyn Fn(f64) -> f64, source: &dyn Fn(f64) -> f64| -> Vec<f64> {
        (0..=6)
            .map(|step| {
                let t = step as f64;
                if step == 0 {
                    0.0
                } else {
                    (partial(t) - z(t - 1.0)) / (z(t) - z(t - 1.0)).abs()
                        * (source(t) - source(t - 1.0)).signum()
                }
            })
            .collect()
    };
    let from_x = edge(&replay, "x", "z", Lag::Current);
    assert_series(
        &score_series(&mut replay, from_x),
        &want(&|t| x(t) * w(t - 1.0), &x),
    );
    let from_w = edge(&replay, "w", "z", Lag::Current);
    assert_series(
        &score_series(&mut replay, from_w),
        &want(&|t| x(t - 1.0) * w(t), &w),
    );
}

#[test]
fn a_score_is_zero_where_either_end_does_not_change() {
    // `held` stops at time 3; `z` stands still while it is at its bound.
    let project = TestProject::new("bounds")
        .with_sim_time(0.0, 6.0, 1.0)
        .aux("held", "MIN(TIME, 3)", None)
        .aux("z", "MAX(held, 1) + 0 * TIME", None);
    let mut replay = replay(&project, Reducers::Inline);
    let e = edge(&replay, "held", "z", Lag::Current);
    assert_series(
        &score_series(&mut replay, e),
        &[0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0],
    );
}

#[test]
fn a_flows_score_is_its_share_of_the_stocks_net_flow() {
    let project = TestProject::new("flows")
        .with_sim_time(0.0, 4.0, 1.0)
        .stock("s", "100", &["births", "arrivals"], &["deaths"], None)
        .flow("births", "s * 0.1", None)
        .flow("arrivals", "5 * TIME", None)
        .flow("deaths", "s * 0.02", None)
        // A stock nothing flows into: its folded update has no net.
        .stock("still", "3", &[], &[], None);
    let mut replay = replay(&project, Reducers::Inline);
    let series = |name: &str| -> Vec<f64> {
        let slot = slot(&replay, name);
        (0..5).map(|step| replay.value(slot, step)).collect()
    };
    let (births, arrivals, deaths) = (series("births"), series("arrivals"), series("deaths"));
    let net: Vec<f64> = (0..5)
        .map(|t| births[t] + arrivals[t] - deaths[t])
        .collect();
    for (name, flow, direction) in [
        ("births", &births, 1.0),
        ("arrivals", &arrivals, 1.0),
        ("deaths", &deaths, -1.0),
    ] {
        let want: Vec<f64> = (0..5)
            .map(|t| {
                if t == 0 {
                    0.0
                } else {
                    direction * ((flow[t] - flow[t - 1]) / (net[t] - net[t - 1])).abs()
                }
            })
            .collect();
        let e = edge(&replay, name, "s", Lag::Current);
        assert!(replay.edges()[e].is_flow_to_stock());
        assert_series(&score_series(&mut replay, e), &want);
        assert_eq!(replay.score_under(e, 2, Convention::ChangedFirst), None);
    }
}

/// A model with every way a program reads a slot in it.
fn every_read() -> TestProject {
    TestProject::new("reads")
        .with_sim_time(0.0, 8.0, 1.0)
        .indexed_dimension("D", 3)
        .indexed_dimension("E", 4)
        .stock("s", "10", &["inflow"], &[], None)
        .flow("inflow", "s * 0.1 + TIME", None)
        .aux("which", "1 + (INT(TIME) MOD 3)", None)
        .array_aux("x[D]", "D * TIME + s")
        .array_aux("y[D]", "10 - D * s")
        .array_aux("m[D,E]", "D * TIME + E * s")
        // A variable, the clock, a lag of each depth, an initial value.
        .aux("scalar", "s * TIME", None)
        .aux("lagged", "PREVIOUS(scalar) + PREVIOUS(PREVIOUS(s))", None)
        .aux("initial", "s - INIT(scalar)", None)
        // A module's inputs.
        .aux("smooth", "SMTH1(scalar, 2) + DELAY3(s, 3)", None)
        // Elements by a fixed and by a computed subscript.
        .aux("fixed", "x[2] - y[1]", None)
        .aux("computed", "x[which] + m[which, which]", None)
        // Whole arrays, slices, and a slice picked as the model runs.
        .aux("whole", "SUM(x[*]) + SUM(m[*, 2]) + SUM(m[1, *])", None)
        .aux("ranged", "SUM(x[which:3])", None)
        .array_aux("share[D]", "x[D] / SUM(x[*])")
        .aux(
            "reduced",
            "1 + MAX(x[*]) + MIN(x[*]) + MEAN(x[*]) + STDDEV(x[*]) + SIZE(x[*])",
            None,
        )
        // Computed arrays, through a temporary.
        .aux("sum_of_products", "SUM(x[*] * y[*])", None)
        .array_aux("ranks[D]", "RANK(y[*], 1) + x[D]")
        .array_aux("order[D]", "VECTOR SORT ORDER(x[*] * y[*], 1)")
        // A view that is read past its end.
        .array_aux("offsets[D]", "D MOD 3")
        .array_aux("mapped[D]", "VECTOR ELM MAP(m[1, 1], offsets[D] + which)")
        .array_aux("mapped_from[D]", "VECTOR ELM MAP(x[which], offsets[D])")
        // Arrays of the step before and of the start.
        .array_aux("order_before[D]", "VECTOR SORT ORDER(PREVIOUS(x[*], 0), 1)")
        .array_aux("order_at_start[D]", "VECTOR SORT ORDER(INIT(x[*]), 1)")
}

#[test]
fn every_slot_is_given_back_by_its_own_code_over_its_reads_alone() {
    let sim = every_read().compile_incremental().expect("compiles");

    // The fixture is the rows of `Cutter::unit`'s reading arms: each opcode
    // that can read the model's slots, and each region a view can be over.
    let mut opcodes: HashSet<&str> = HashSet::new();
    let mut regions: Vec<ViewStorage> = Vec::new();
    for module in sim.modules.values() {
        for op in &module.compiled_flows.code {
            opcodes.insert(op.name());
            if let Opcode::PushStaticView { view_id } = op {
                regions.push(module.context.static_views[*view_id as usize].storage);
            }
        }
    }
    for reading in [
        "LoadVar",
        "LoadGlobalVar",
        "LoadPrev",
        "LoadModuleInput",
        "LoadSubscript",
        "PushStaticView",
        "PushVarViewDirect",
        "VectorElmMap",
    ] {
        assert!(opcodes.contains(reading), "no {reading} in {opcodes:?}");
    }
    for region in [
        ViewStorage::Curr,
        ViewStorage::Prev,
        ViewStorage::Initial,
        ViewStorage::Temp,
    ] {
        assert!(regions.contains(&region), "no view over {region:?}");
    }

    for reducers in [Reducers::Inline, Reducers::AsNodes] {
        let mut replay = Replay::new(&sim, reducers).expect("the program is cut");
        let found = replay.misreproduced();
        assert!(found.is_empty(), "{reducers:?}: {found:#?}");
    }
}

#[test]
fn a_read_that_is_missed_is_found() {
    let sim = every_read().compile_incremental().expect("compiles");
    let mut replay = Replay::new(&sim, Reducers::AsNodes).expect("the program is cut");
    // Every unit's every read in turn, less the ones that can make no
    // difference in this run: an element a computed subscript never picks, and
    // the array `SIZE` counts.
    let mut unfound: Vec<String> = Vec::new();
    for unit in 0..replay.units.len() {
        for read in 0..replay.units[unit].reads.len() {
            let missed = replay.units[unit].reads.remove(read);
            let target = replay.name(replay.units[unit].target).map(str::to_string);
            let found = replay.misreproduced();
            if !found.iter().any(|m| Some(&m.name) == target.as_ref()) {
                unfound.push(format!(
                    "{} -> {}",
                    replay.name(missed.slot).unwrap_or("?"),
                    target.unwrap_or_default()
                ));
            }
            replay.units[unit].reads.insert(read, missed);
        }
    }
    let makes_no_difference = |link: &str| {
        ["computed", "ranged", "mapped[", "mapped_from[", "reduced"]
            .iter()
            .any(|target| link.contains(&format!("-> {target}")))
    };
    unfound.retain(|link| !makes_no_difference(link));
    // The slots without a name are the capture helpers', whose misses are
    // found through the variables that read them.
    unfound.retain(|link| !link.ends_with("-> "));
    assert!(unfound.is_empty(), "{unfound:#?}");
}

#[test]
fn a_reducer_is_a_node_of_its_own_or_part_of_its_equation() {
    let project = TestProject::new("shares")
        .with_sim_time(0.0, 4.0, 1.0)
        .indexed_dimension("D", 3)
        .array_aux("x[D]", "D + TIME * D * D")
        .array_aux("share[D]", "x[D] / SUM(x[*])")
        // The same reducer again, the second time a variable's whole equation.
        .aux("twice", "2 * SUM(x[*])", None)
        .aux("total", "SUM(x[*])", None);
    let x = |d: f64, t: f64| d + t * d * d;
    let sum = |t: f64| x(1.0, t) + x(2.0, t) + x(3.0, t);
    let share = |t: f64| x(1.0, t) / sum(t);

    let mut inline = replay(&project, Reducers::Inline);
    let e = edge(&inline, "x[2]", "share[1]", Lag::Current);
    let want: Vec<f64> = (0..5)
        .map(|step| {
            let t = step as f64;
            if step == 0 {
                return 0.0;
            }
            let partial = x(1.0, t - 1.0) / (sum(t - 1.0) - x(2.0, t - 1.0) + x(2.0, t));
            (partial - share(t - 1.0)) / (share(t) - share(t - 1.0)).abs()
        })
        .collect();
    assert_series(&score_series(&mut inline, e), &want);

    let mut nodes = replay(&project, Reducers::AsNodes);
    let node = slot(&nodes, "<sum 0>");
    assert!(nodes.is_reducer_node(node));
    assert_eq!(
        nodes.width(),
        nodes.n_slots + 1,
        "one reducer, written thrice"
    );
    assert_eq!(nodes.value(node, 3), sum(3.0));
    let into = edge(&nodes, "x[2]", "<sum 0>", Lag::Current);
    let want: Vec<f64> = (0..5)
        .map(|step| {
            let t = step as f64;
            if step == 0 {
                return 0.0;
            }
            (x(2.0, t) - x(2.0, t - 1.0)) / (sum(t) - sum(t - 1.0))
        })
        .collect();
    assert_series(&score_series(&mut nodes, into), &want);
    edge(&nodes, "<sum 0>", "share[1]", Lag::Current);
    edge(&nodes, "<sum 0>", "twice", Lag::Current);
    edge(&nodes, "<sum 0>", "total", Lag::Current);
    for target in ["share[1]", "twice", "total"] {
        edge(&inline, "x[2]", target, Lag::Current);
        let (x2, target) = (slot(&nodes, "x[2]"), slot(&nodes, target));
        assert!(
            !nodes.edges().iter().any(|e| (e.from, e.to) == (x2, target)),
            "x[2] reaches it through the node alone"
        );
    }
}

#[test]
fn a_lagged_source_is_scored_by_its_change_a_step_earlier() {
    let project = TestProject::new("lag")
        .with_sim_time(0.0, 5.0, 1.0)
        .aux("x", "TIME * TIME - 4 * TIME", None)
        .aux("z", "3 * PREVIOUS(x, 7)", None);
    let mut replay = replay(&project, Reducers::Inline);
    let e = edge(&replay, "x", "z", Lag::Previous);
    // x: 0, -3, -4, -3, 0, 5; z: 21, 0, -9, -12, -9, 0. The first change of
    // z is its fallback's, which x has no part in.
    assert_series(
        &score_series(&mut replay, e),
        &[0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
    );
}

#[test]
fn the_inside_of_a_module_instance_is_part_of_the_model() {
    let project = TestProject::new("smooth")
        .with_sim_time(0.0, 5.0, 1.0)
        .aux("x", "TIME * TIME", None)
        .aux("z", "SMTH1(x, 3) * 2", None);
    let mut replay = replay(&project, Reducers::Inline);
    let prefixes: Vec<&str> = replay
        .instances()
        .iter()
        .map(|i| i.prefix.as_str())
        .collect();
    let [root, instance] = prefixes.as_slice() else {
        panic!("{prefixes:?}");
    };
    assert_eq!(*root, "");
    assert!(instance.ends_with('\u{B7}'), "{instance}");
    let instance = instance.to_string();

    // The port holds what it is bound to; the code inside reads the source.
    let port = slot(&replay, &format!("{instance}input"));
    assert_eq!(replay.port_source(port), Some(slot(&replay, "x")));
    assert_eq!(replay.port_source(slot(&replay, "z")), None);
    let flow = format!("{instance}flow");
    edge(&replay, "x", &flow, Lag::Current);
    let into_stock = edge(&replay, &flow, &format!("{instance}output"), Lag::Current);
    assert!(replay.edges()[into_stock].is_flow_to_stock());
    let out = edge(&replay, &format!("{instance}output"), "z", Lag::Current);
    assert_eq!(score_series(&mut replay, out)[3], 1.0);
    assert!(replay.misreproduced().is_empty());
}

#[test]
fn a_run_is_kept_at_every_step_and_says_which_it_saves() {
    let project = TestProject::new("saved")
        .with_sim_time(0.0, 4.0, 0.5)
        .with_save_step(2.0)
        .aux("x", "TIME", None);
    let replay = replay(&project, Reducers::Inline);
    assert_eq!(replay.n_steps(), 9);
    assert_eq!(replay.saved_steps(), [0, 4, 8]);
    assert_eq!(replay.value(slot(&replay, "x"), 3), 1.5);
}

/// `z` is of two sources, the clock and a lag, so no two conventions agree.
fn conventions_project() -> TestProject {
    TestProject::new("conventions")
        .with_sim_time(0.0, 8.0, 1.0)
        .indexed_dimension("D", 2)
        .array_aux("x[D]", "(TIME - 3) * (TIME - 3) * D + 1")
        .aux("w", "5 * SIN(TIME) + 9", None)
        .aux("z", "x[1] * w * TIME + x[2] * w + 10 * PREVIOUS(w)", None)
}

#[test]
fn each_convention_is_the_number_it_states() {
    let mut replay = replay(&conventions_project(), Reducers::Inline);
    let series = |name: &str| -> Vec<f64> {
        let slot = slot(&replay, name);
        (0..9).map(|step| replay.value(slot, step)).collect()
    };
    let (x1, x2, w, z) = (series("x[1]"), series("x[2]"), series("w"), series("z"));
    let f =
        |x1: f64, x2: f64, w: f64, time: f64, lagged: f64| x1 * w * time + x2 * w + 10.0 * lagged;
    let current = edge(&replay, "x[1]", "z", Lag::Current);
    let lagged = edge(&replay, "w", "z", Lag::Previous);
    use Convention::*;
    for t in 2..9 {
        let (now, before) = (t as f64, t as f64 - 1.0);
        let dz = (z[t] - z[t - 1]).abs();
        let sign_of = |v: &[f64], t: usize| (v[t] - v[t - 1]).signum();
        // x[1] -> z, the source read this step.
        let first = f(x1[t], x2[t - 1], w[t - 1], before, w[t - 2]) - z[t - 1];
        let last = z[t] - f(x1[t - 1], x2[t], w[t], now, w[t - 1]);
        let live_clock = f(x1[t], x2[t - 1], w[t - 1], now, w[t - 2]) - z[t - 1];
        let live_lags = f(x1[t], x2[t - 1], w[t - 1], before, w[t - 1]) - z[t - 1];
        let both_first = f(x1[t], x2[t], w[t - 1], before, w[t - 2]) - z[t - 1];
        let both_last = z[t] - f(x1[t - 1], x2[t - 1], w[t], now, w[t - 1]);
        let s = sign_of(&x1, t);
        for convention in Convention::ALL {
            let want = match convention {
                ChangedFirst => first / dz * s,
                ChangedLast => last / dz * s,
                LiveClock => live_clock / dz * s,
                LiveLags | LiveLagsCurrentSign => live_lags / dz * s,
                WholeChange => (z[t] - z[t - 1]) / dz * s,
                WholeSource => (both_first / dz).abs(),
                WholeSourceChangedLast => (both_last / dz).abs(),
            };
            let got = replay.score_under(current, t, convention);
            assert!(
                got.is_some_and(|got| (got - want).abs() <= 1e-9 * want.abs().max(1.0)),
                "{convention:?} at step {t}: {got:?} is not {want}"
            );
        }
        // w -> z through `PREVIOUS`: the lagged read moves, the live one not.
        let first = f(x1[t - 1], x2[t - 1], w[t - 1], before, w[t - 1]) - z[t - 1];
        let last = z[t] - f(x1[t], x2[t], w[t], now, w[t - 2]);
        for (convention, want) in [
            (ChangedFirst, first / dz * sign_of(&w, t - 1)),
            (ChangedLast, last / dz * sign_of(&w, t - 1)),
            (LiveLags, first / dz * sign_of(&w, t - 1)),
            (LiveLagsCurrentSign, first / dz * sign_of(&w, t)),
        ] {
            let got = replay.score_under(lagged, t, convention);
            assert!(
                got.is_some_and(|got| (got - want).abs() <= 1e-9 * want.abs().max(1.0)),
                "lagged {convention:?} at step {t}: {got:?} is not {want}"
            );
        }
        // What `scores` answers is the definition.
        for e in [current, lagged] {
            let series = replay.scores(&[t]);
            assert_eq!(
                replay.score_under(e, t, ChangedFirst).map(f64::to_bits),
                Some(series[e].to_bits())
            );
        }
    }
    for convention in [ChangedFirst, WholeChange] {
        assert_eq!(replay.score_under(current, 0, convention), Some(0.0));
    }
}

#[test]
fn a_program_with_no_rule_for_it_is_refused() {
    let project = TestProject::new("refused")
        .with_sim_time(0.0, 3.0, 1.0)
        .aux("x", "TIME", None)
        .aux("y", "x * 2 * x / x", None)
        .aux("z", "x * 2 + x", None);
    let sim = project.compile_incremental().expect("compiles");
    let refusal = |change: &dyn Fn(&mut CompiledModule)| -> String {
        let mut sim = (*sim).clone();
        let root = sim.root.clone();
        change(sim.modules.get_mut(&root).expect("the root module"));
        match Replay::new(&Arc::new(sim), Reducers::Inline) {
            Ok(_) => "nothing".to_string(),
            Err(e) => e,
        }
    };
    // The VM's own fused copy of a program, whose opcodes this has no rules
    // for: one that is added to codegen fails here the same way.
    let fused = refusal(&|module| Arc::make_mut(&mut module.compiled_flows).fuse_three_address());
    assert!(fused.contains("no rule for what"), "{fused}");
    let fused_last = refusal(&|module| {
        let flows = Arc::make_mut(&mut module.compiled_flows);
        let last = flows
            .code
            .iter()
            .rposition(|op| matches!(op, Opcode::BinOpAssignCurr { .. }))
            .expect("z's assignment");
        flows.code[last] = Opcode::AssignStackConstCurr {
            dst: 4,
            b: 0,
            op: Op2::Add,
        };
    });
    assert!(
        fused_last.contains("no rule for what the code"),
        "{fused_last}"
    );
    let twice = refusal(&|module| {
        let flows = Arc::make_mut(&mut module.compiled_flows);
        let ret = flows.code.pop().expect("a Ret");
        let again = flows.code.clone();
        flows.code.extend(again);
        flows.code.push(ret);
    });
    assert!(twice.contains("computed in two places"), "{twice}");
    let not_a_sum = refusal(&|module| {
        let stocks = Arc::make_mut(&mut module.compiled_stocks);
        stocks.code.insert(0, Opcode::Not {});
    });
    assert!(not_a_sum.contains("not a sum of flows"), "{not_a_sum}");

    let context = ByteCodeContext::default();
    let cut_refusal = |code: &[Opcode]| cut(code, &context).err().unwrap_or_default();
    let reads = Opcode::LoadTempConst {
        temp_id: 0,
        index: 0,
    };
    let assign = Opcode::AssignCurr { off: 4 };
    assert!(cut_refusal(&[reads, assign]).contains("which nothing has filled"));
    assert!(
        cut_refusal(&[
            Opcode::PushVarViewDirect {
                base_off: 4,
                dim_list_id: 0
            },
            Opcode::LoadVar { off: 4 },
            assign
        ])
        .contains("leaves a stack in use")
    );
    assert!(
        cut_refusal(&[
            Opcode::LoadVar { off: 4 },
            Opcode::LoadVar { off: 5 },
            Opcode::Op2 { op: Op2::Add },
            Opcode::EvalModule { id: 0, n_inputs: 1 }
        ])
        .contains("an input is computed")
    );
    let begin = |write_temp_id| Opcode::BeginIter {
        write_temp_id,
        has_write_temp: true,
    };
    assert!(cut_refusal(&[begin(0), begin(1)]).contains("a second temp in one piece"));
}

#[test]
fn one_load_of_several_moves_alone() {
    let project = TestProject::new("loads")
        .with_sim_time(0.0, 6.0, 1.0)
        .aux("x", "TIME * TIME + 1", None)
        .aux("z", "x * 10 + x * x + PREVIOUS(x) * x", None)
        .aux("y", "PREVIOUS(x) + 3 * PREVIOUS(x)", None);
    let mut replay = replay(&project, Reducers::Inline);
    let x = |t: f64| t * t + 1.0;
    let current = edge(&replay, "x", "z", Lag::Current);
    let lagged = edge(&replay, "x", "y", Lag::Previous);
    assert_eq!((replay.loads(current), replay.loads(lagged)), (4, 2));
    for step in 2..7 {
        let t = step as f64;
        let (now, before, older) = (x(t), x(t - 1.0), x(t - 2.0));
        let z = |t: f64| x(t) * 10.0 + x(t) * x(t) + x(t - 1.0) * x(t);
        // The four loads of x, each at its new value with the rest held.
        for (load, moved) in [10.0, before, before, older].into_iter().enumerate() {
            let want = moved * (now - before) / (z(t) - z(t - 1.0));
            let got = replay.score_of_one_load(current, step, load);
            assert!(
                got.is_some_and(|got| (got - want).abs() < 1e-9),
                "load {load} at step {step}: {got:?} is not {want}"
            );
        }
        for (load, share) in [0.25, 0.75].into_iter().enumerate() {
            let got = replay.score_of_one_load(lagged, step, load);
            assert!(
                got.is_some_and(|got| (got - share).abs() < 1e-9),
                "lagged load {load} at step {step}: {got:?} is not {share}"
            );
        }
        assert_eq!(replay.score_of_one_load(current, step, 4), None);
    }
    assert_eq!(replay.score_of_one_load(current, 0, 0), Some(0.0));
}
