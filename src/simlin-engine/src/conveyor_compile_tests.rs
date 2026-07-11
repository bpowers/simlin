// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for conveyor expansion/compilation ([`super`]). Split out of
//! `conveyor_compile.rs` to keep that file under the project line-count
//! lint; this is the `#[cfg(test)] mod tests` body, included via `#[path]`
//! so `use super::*` still resolves the module's private items.

use super::*;
use crate::common::Ident;
// The build entry points live in queue_compile (the unified conveyor+queue
// build path); these tests pin the conveyor half of its behavior.
use crate::queue_compile::{build_compiled_fresh, build_vm};
use std::io::BufReader;

fn parse(xml: &str) -> datamodel::Project {
    crate::xmile::project_from_reader(&mut BufReader::new(xml.as_bytes())).unwrap()
}

#[test]
fn minimal_conveyor_simulates_steady_state() {
    // init Students=1000 == inflow(250) * transit(4) == steady state, so the
    // whole run should hold flat: Students=1000, graduating=250, and Alumni
    // accumulates 250/time.
    let xml = include_str!("../../../test/conveyors/minimal_conveyor.xmile");
    let project = parse(xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build conveyor vm");
    vm.run_to_end().expect("run");

    let students = vm
        .get_series(&Ident::new("students"))
        .expect("students series");
    let graduating = vm
        .get_series(&Ident::new("graduating"))
        .expect("graduating series");
    let alumni = vm.get_series(&Ident::new("alumni")).expect("alumni series");
    assert!(students.len() > 40, "should have many saved steps");
    for (i, &s) in students.iter().enumerate() {
        assert!(
            (s - 1000.0).abs() < 1e-6,
            "step {i}: Students={s} (want 1000)"
        );
    }
    // graduating is a during-step flow rate; steady at 250.
    for (i, &g) in graduating.iter().enumerate().skip(1) {
        assert!(
            (g - 250.0).abs() < 1e-6,
            "step {i}: graduating={g} (want 250)"
        );
    }
    // Alumni accumulates the outflow: rises monotonically to 250*12 = 3000.
    assert!(
        (alumni[alumni.len() - 1] - 3000.0).abs() < 1.0,
        "final Alumni {}",
        alumni[alumni.len() - 1]
    );
}

#[test]
fn fill_from_empty_is_a_transit_delay() {
    // Same model but Students starts empty: outflow stays 0 until the belt
    // fills (transit=4), then equals the inflow (pure T-unit delay, S2).
    let xml = include_str!("../../../test/conveyors/minimal_conveyor.xmile")
        .replace("<eqn>1000</eqn>", "<eqn>0</eqn>");
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build");
    vm.run_to_end().expect("run");
    let students = vm.get_series(&Ident::new("students")).expect("students");
    let graduating = vm
        .get_series(&Ident::new("graduating"))
        .expect("graduating");
    // dt=0.25, transit=4 -> 16 slats; the first inflow inserted at t=0 exits
    // at t=4.0, i.e. the outflow is 0 for the first 16 steps.
    assert_eq!(graduating[0], 0.0);
    for (i, &g) in graduating.iter().enumerate().take(16) {
        assert!(
            g.abs() < 1e-9,
            "step {i}: outflow should be 0 during fill, got {g}"
        );
    }
    // once full, outflow reaches the inflow rate.
    assert!(
        (graduating[20] - 250.0).abs() < 1e-6,
        "step 20 outflow {}",
        graduating[20]
    );
    assert!(
        (students[16] - 1000.0).abs() < 1e-6,
        "belt full at step 16: {}",
        students[16]
    );
}

fn wrap_model(vars: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
<options><uses_conveyor/></options></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>20</stop><dt>0.25</dt></sim_specs>
  <model><variables>{vars}</variables></model>
</xmile>"#
    )
}

#[test]
fn capacity_plateaus_contents() {
    // S5: capacity=600, inflow 250, transit 4 -> contents plateau at 600.
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>4</len><capacity>600</capacity></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build");
    vm.run_to_end().expect("run");
    let belt = vm.get_series(&Ident::new("belt")).expect("belt");
    for (i, &b) in belt.iter().enumerate() {
        assert!(b <= 600.0 + 1e-6, "step {i}: contents {b} exceeds capacity");
    }
    assert!(
        (belt[belt.len() - 1] - 600.0).abs() < 1e-6,
        "plateaus at 600: {}",
        belt[belt.len() - 1]
    );
}

#[test]
fn linear_leak_reaches_steady_state() {
    // S3: linear leak f=0.2, inflow 250, transit 4 -> steady outflow 200,
    // leak 50 (init empty, run long enough to settle).
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>attriting</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <flow name="attriting"><eqn>0.2</eqn><leak/></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build");
    vm.run_to_end().expect("run");
    let out = vm.get_series(&Ident::new("out_f")).expect("out_f");
    let leak = vm.get_series(&Ident::new("attriting")).expect("attriting");
    let last = out.len() - 1;
    assert!(
        (out[last] - 200.0).abs() < 1e-4,
        "steady outflow {} (want 200)",
        out[last]
    );
    assert!(
        (leak[last] - 50.0).abs() < 1e-4,
        "steady leak {} (want 50)",
        leak[last]
    );
}

/// The leak model shared by the GH #871 override tests: primary outflow
/// `out_f`, leak `attriting` (f=0.2), plus a container-access aux so a
/// synthesized container stock (`$conv$sum$belt`) exists.
fn override_leak_model() -> String {
    wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>attriting</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <flow name="attriting"><eqn>0.2</eqn><leak/></flow>
    <aux name="on_belt"><eqn>SUM(belt)</eqn></aux>"#,
    )
}

#[test]
fn set_value_on_pass_driven_conveyor_slots_rejected() {
    // GH #871: expansion rewrites every conveyor-driven flow to a
    // placeholder `0`, which compiles to an overridable AssignConstCurr.
    // Without the pass-driven exclusion, set_value on the primary outflow
    // or a leak silently succeeds -- but the conveyor pass overwrites the
    // slot every step, so the override never affects the simulation. It
    // must instead be rejected like any other computed flow. The container
    // stock's slot is pass-published too and is rejected for the same
    // reason (a stock was never overridable; pinning it here keeps the
    // "every pass-written slot rejects" invariant explicit).
    let project = parse(&override_leak_model());
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build");

    for name in ["out_f", "attriting", "$conv$sum$belt"] {
        let err = vm.set_value(&Ident::new(name), 999.0).unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::BadOverride,
            "set_value('{name}') must be rejected"
        );
    }

    // The rejected overrides leave no trace: the run is belt-driven
    // (S3 steady state: out 200, leak 50) and get_series reports it.
    vm.run_to_end().expect("run");
    let out = vm.get_series(&Ident::new("out_f")).expect("out_f");
    let leak = vm.get_series(&Ident::new("attriting")).expect("attriting");
    let last = out.len() - 1;
    assert!(
        (out[last] - 200.0).abs() < 1e-4,
        "steady outflow {} (want 200)",
        out[last]
    );
    assert!(
        (leak[last] - 50.0).abs() < 1e-4,
        "steady leak {} (want 50)",
        leak[last]
    );
}

#[test]
fn set_value_on_equation_driven_inflow_remains_effective() {
    // An equation-driven inflow is a genuine pass INPUT: the Flows phase
    // computes the requested rate and the pass admits against it (writing
    // back only the admitted rate). A constant-inflow override must
    // therefore stay accepted AND change the simulation -- this pins that
    // the GH #871 rejection covers only pass-WRITTEN slots, not pass
    // inputs. With in_f overridden to 100 and leak f=0.2, the S3 steady
    // state scales to out 80 / leak 20.
    let project = parse(&override_leak_model());
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build");
    vm.set_value(&Ident::new("in_f"), 100.0)
        .expect("inflow override must be accepted");
    vm.run_to_end().expect("run");
    let out = vm.get_series(&Ident::new("out_f")).expect("out_f");
    let leak = vm.get_series(&Ident::new("attriting")).expect("attriting");
    let last = out.len() - 1;
    assert!(
        (out[last] - 80.0).abs() < 1e-4,
        "steady outflow {} (want 80 = 100 * 0.8)",
        out[last]
    );
    assert!(
        (leak[last] - 20.0).abs() < 1e-4,
        "steady leak {} (want 20 = 100 * 0.2)",
        leak[last]
    );
}

#[test]
fn mid_run_get_value_matches_saved_series() {
    // After a PARTIAL run (run_to to a mid-simulation time), the #625
    // resting-curr re-eval re-runs the Flows phase to make the live curr
    // chunk self-consistent for mid-run inspection. For a conveyor model
    // that re-eval used to re-execute each pass-driven flow's placeholder
    // `AssignConstCurr 0`, so get_value_now of the primary outflow / a
    // leak read 0 instead of the pass-computed rate, and the published
    // container values were one step stale. The contract pinned here: the
    // resting chunk holds EXACTLY the values the resumed run recomputes
    // and saves for the same time (dt == save_step == 0.25, so resting
    // times 2.25 / 10.25 are saved rows 9 / 41).
    let project = parse(&override_leak_model());
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build");

    let names = [
        "out_f",
        "attriting",
        "in_f",
        "on_belt",
        "$conv$sum$belt",
        "belt",
    ];
    let offs: Vec<usize> = names
        .iter()
        .map(|n| vm.get_offset(&Ident::new(n)).expect("offset"))
        .collect();

    // Rest mid-fill (t=2.25): the belt is still filling, so the container
    // total is transient (catching a stale one-step-old publish) and the
    // leak is nonzero while the primary outflow is still 0.
    vm.run_to(2.0).expect("run_to 2");
    let mid_fill: Vec<f64> = offs.iter().map(|&o| vm.get_value_now(o)).collect();
    assert!(
        mid_fill[1] > 0.0,
        "attriting {} must be belt-driven (> 0) mid-fill",
        mid_fill[1]
    );

    // Rest at steady state (t=10.25): out 200, leak 50 (S3).
    vm.run_to(10.0).expect("run_to 10");
    let mid_steady: Vec<f64> = offs.iter().map(|&o| vm.get_value_now(o)).collect();
    assert!(
        (mid_steady[0] - 200.0).abs() < 1e-4,
        "mid-run out_f {} (want 200)",
        mid_steady[0]
    );
    assert!(
        (mid_steady[1] - 50.0).abs() < 1e-4,
        "mid-run attriting {} (want 50)",
        mid_steady[1]
    );

    vm.run_to_end().expect("run");
    for (i, name) in names.iter().enumerate() {
        let series = vm.get_series(&Ident::new(name)).expect(name);
        assert_eq!(
            series[9], mid_fill[i],
            "{name}: mid-fill read {} != saved row {}",
            mid_fill[i], series[9]
        );
        assert_eq!(
            series[41], mid_steady[i],
            "{name}: steady read {} != saved row {}",
            mid_steady[i], series[41]
        );
    }
}

#[test]
fn arrayed_mid_run_get_value_reads_per_element_rates() {
    // The arrayed twin of `mid_run_get_value_matches_saved_series`: each
    // element belt has its own pass-written slot, and at t=3.25 the two
    // belts are in DIFFERENT states (belt[a], transit 2, is already
    // steady at out 100; belt[b], transit 4, is still filling at out 0)
    // -- so a placeholder-zero read is distinguishable per element.
    let xml = include_str!("../../../test/conveyors/arrayed_conveyor.xmile");
    let project = parse(xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build arrayed conveyor vm");

    let names = ["outflow_f[a]", "outflow_f[b]", "belt[a]", "belt[b]"];
    let offs: Vec<usize> = names
        .iter()
        .map(|n| vm.get_offset(&Ident::new(n)).expect("offset"))
        .collect();

    vm.run_to(3.0).expect("run_to 3");
    let mid: Vec<f64> = offs.iter().map(|&o| vm.get_value_now(o)).collect();
    assert!(
        (mid[0] - 100.0).abs() < 1e-6,
        "mid-run outflow_f[a] {} (want 100, steady)",
        mid[0]
    );
    assert!(
        mid[1].abs() < 1e-9,
        "mid-run outflow_f[b] {} (want 0, still filling)",
        mid[1]
    );

    // Resting time 3.25 is saved row 13 (dt == save_step == 0.25).
    vm.run_to_end().expect("run");
    for (i, name) in names.iter().enumerate() {
        let series = vm.get_series(&Ident::new(name)).expect(name);
        assert_eq!(
            series[13], mid[i],
            "{name}: mid-run read {} != saved row {}",
            mid[i], series[13]
        );
    }
}

/// Assert that a run segmented by mid-run rests produces BIT-identical
/// saved series to an uninterrupted run, for EVERY variable. This is the
/// strongest direct pin that the #625 resting-curr pass preview has no
/// side effect on the real belt/FIFO side tables (no double-advance): a
/// preview that mutated real state would shift every subsequent row.
fn assert_segmented_run_identical(project: &datamodel::Project, rests: &[f64]) {
    let main = project.models[0].name.clone();
    let mut seg = build_vm(project, &main).expect("build segmented vm");
    for &t in rests {
        seg.run_to(t).expect("segmented run_to");
    }
    seg.run_to_end().expect("segmented run_to_end");
    let mut full = build_vm(project, &main).expect("build full vm");
    full.run_to_end().expect("full run_to_end");

    for name in full.names_as_strs() {
        let ident = Ident::new(&name);
        let a = full.get_series(&ident).expect("full series");
        let b = seg.get_series(&ident).expect("segmented series");
        assert_eq!(a.len(), b.len(), "{name}: series length");
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            assert_eq!(
                x.to_bits(),
                y.to_bits(),
                "{name} step {i}: full {x} != segmented {y}"
            );
        }
    }
}

#[test]
fn segmented_run_is_bit_identical_to_uninterrupted() {
    // Conveyor with a leak and a container-access aux: two mid-run rests
    // (one mid-fill, one at steady state) must leave the belt exactly as
    // an uninterrupted run left it.
    let project = parse(&override_leak_model());
    assert_segmented_run_identical(&project, &[2.0, 10.0]);
}

#[test]
fn mid_run_resting_values_reflect_inflow_override() {
    // An accepted override on an equation-driven INFLOW is a genuine pass
    // input: the resting-curr re-eval recomputes the Flows phase with the
    // overridden literal and the pass consumes it, so a mid-run read of
    // the pass-driven slots reflects the overridden belt dynamics. With
    // in_f overridden to 500 (leak f=0.2), the S3 steady state scales to
    // out 400 / leak 100, reached well before the t=10.25 resting point.
    let project = parse(&override_leak_model());
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build");
    vm.set_value(&Ident::new("in_f"), 500.0)
        .expect("inflow override must be accepted");
    vm.run_to(10.0).expect("run_to 10");
    let in_f = vm.get_value_now(vm.get_offset(&Ident::new("in_f")).unwrap());
    let out = vm.get_value_now(vm.get_offset(&Ident::new("out_f")).unwrap());
    let leak = vm.get_value_now(vm.get_offset(&Ident::new("attriting")).unwrap());
    assert_eq!(in_f, 500.0, "resting in_f must read the override");
    assert!(
        (out - 400.0).abs() < 1e-4,
        "mid-run out_f {out} (want 400 = 500 * 0.8)"
    );
    assert!(
        (leak - 100.0).abs() < 1e-4,
        "mid-run attriting {leak} (want 100 = 500 * 0.2)"
    );
}

#[test]
fn directly_assembled_vm_rejects_pass_driven_overrides() {
    // Defense-in-depth for GH #871: a Vm assembled by hand from an
    // UNSCRUBBED CompiledSimulation (bypassing build_compiled's exclusion)
    // must still reject pass-driven slots, because set_conveyor_plans
    // itself retracts them from the overridable-constant set.
    let project = parse(&override_leak_model());
    let main = project.models[0].name.clone();
    let (expanded, metas) = expand_conveyors(&project, &main).expect("expand");
    let mut db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel_incremental(&mut db, &expanded, None);
    let compiled =
        crate::db::compile_project_incremental(&db, sync.project, &main).expect("compile");
    let plans = resolve_plans(&metas, &compiled.offsets).expect("resolve");
    let mut vm = crate::vm::Vm::new(compiled).expect("vm");
    vm.set_conveyor_plans(plans);
    let err = vm.set_value(&Ident::new("out_f"), 999.0).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadOverride);
    let err = vm.set_value(&Ident::new("attriting"), 999.0).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadOverride);
}

#[test]
fn duplicate_canonical_leak_flow_expands_without_panic() {
    // Two flows canonicalize to the same ident ('Attrition'/'attrition');
    // the XMILE reader stable-sorts by canonical ident but never dedups,
    // so the leak-LESS twin sits first in document order. Detecting
    // leak-ness with an any() scan over every same-canonical flow but then
    // fetching the FIRST canon-matching flow and unwrapping its leakage
    // panicked here (GH #870); the fetch must select the same flow that
    // made the outflow a leak.
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>Attrition</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <flow name="Attrition"><eqn>1</eqn></flow>
    <flow name="attrition"><eqn>0.2</eqn><leak/></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let (expanded, metas) = expand_conveyors(&project, &main).expect("expand");
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].leaks.len(), 1, "outflow must be treated as a leak");
    assert_eq!(metas[0].leaks[0].flow, "attrition");
    // The synthesized fraction aux must come from the leak-carrying twin's
    // equation (0.2), never the leak-less twin's (1).
    let frac_aux = leak_frac_name("attrition");
    let frac_eqn = expanded.models[0].variables.iter().find_map(|v| match v {
        datamodel::Variable::Aux(a) if a.ident == frac_aux => Some(a.equation.clone()),
        _ => None,
    });
    assert_eq!(frac_eqn, Some(Equation::Scalar("0.2".to_string())));
}

#[test]
fn duplicate_canonical_leak_flow_selects_leak_twin_regardless_of_order() {
    // Same duplicate pair with the leak-carrying twin FIRST in document
    // order. This ordering never panicked, but it must resolve to the
    // identical interpretation as the reversed ordering above: the leak
    // twin supplies the fraction, whichever side of the duplicate it is.
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>Attrition</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <flow name="attrition"><eqn>0.2</eqn><leak/></flow>
    <flow name="Attrition"><eqn>1</eqn></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let (expanded, metas) = expand_conveyors(&project, &main).expect("expand");
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].leaks.len(), 1, "outflow must be treated as a leak");
    assert_eq!(metas[0].leaks[0].flow, "attrition");
    let frac_aux = leak_frac_name("attrition");
    let frac_eqn = expanded.models[0].variables.iter().find_map(|v| match v {
        datamodel::Variable::Aux(a) if a.ident == frac_aux => Some(a.equation.clone()),
        _ => None,
    });
    assert_eq!(frac_eqn, Some(Equation::Scalar("0.2".to_string())));
}

#[test]
fn duplicate_canonical_inflow_spreadflow_marker_twin_wins() {
    // The spreadflow sibling of the two leak-twin tests above: two inflow
    // flows canonicalize to the same ident and only the LATER twin carries
    // `isee:spreadflow`. `resolve_placement` must select the marker-
    // carrying twin -- the same convention `find_leak_flow` established
    // for `<leak/>` (GH #870) -- not whichever twin sorts first. Duplicate
    // idents are rejected upstream at the build chokepoints (GH #885), so
    // this pins expansion's internal self-consistency for direct callers.
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_flow</inflow><outflow>out_f</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="In Flow"><eqn>250</eqn></flow>
    <flow name="in_flow" isee:spreadflow="even"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let (_expanded, metas) = expand_conveyors(&project, &main).expect("expand");
    assert_eq!(metas.len(), 1);
    let inflow = metas[0]
        .inflows
        .iter()
        .find(|i| i.flow == "in_flow")
        .expect("the duplicate-canonical inflow must be resolved");
    assert_eq!(
        inflow.placement,
        Placement::Even,
        "the spreadflow-marked twin must supply the placement"
    );
}

#[test]
fn unexpanded_conveyor_rejected_by_ordinary_compile() {
    // A conveyor model compiled through the ordinary (non-conveyor) path
    // must fail loudly rather than silently integrate the belt as a plain
    // stock. This is the production-path safety guard.
    let xml = include_str!("../../../test/conveyors/minimal_conveyor.xmile");
    let project = parse(xml);
    let mut db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel_incremental(&mut db, &project, None);
    let main = project.models[0].name.clone();
    let err = crate::db::compile_project_incremental(&db, sync.project, &main)
        .expect_err("un-expanded conveyor must be rejected");
    assert_eq!(err.code, ErrorCode::ConveyorNotExpanded);
}

#[test]
fn even_placement_sends_material_to_exit_immediately() {
    // With `even`, an inflow lands A/d at every entry-path slat INCLUDING
    // the exit slat, so material exits on the first step -- unlike the
    // default `beginning` (0 outflow until the belt fills). Steady outflow
    // still equals the inflow (mass conservation, independent of placement).
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f" isee:spreadflow="even"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build even");
    vm.run_to_end().expect("run");
    let out = vm.get_series(&Ident::new("out_f")).expect("out_f");
    assert!(
        out[1] > 0.0,
        "even: material should exit immediately, out[1]={}",
        out[1]
    );
    assert!(
        (out[out.len() - 1] - 250.0).abs() < 1e-4,
        "steady outflow {}",
        out[out.len() - 1]
    );
}

#[test]
fn dist_without_representable_distribution_is_rejected() {
    // `dist` whose <isee:distrib_eq> is empty, an inline expression, or a
    // dangling reference has no representable distribution -- rejected loudly
    // (a silent Beginning fallback would hide a modeling error). `source`,
    // by contrast, is NOT rejected: on an equation-driven inflow it simply
    // degrades to Beginning (there is no upstream leak to mirror).
    for distrib in ["", "in_f * 2", "not_a_variable"] {
        let distrib_tag = if distrib.is_empty() {
            String::new()
        } else {
            format!("<isee:distrib_eq>{distrib}</isee:distrib_eq>")
        };
        let xml = wrap_model(&format!(
            r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f" isee:spreadflow="dist"><eqn>250</eqn>{distrib_tag}</flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#
        ));
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let err = build_vm(&project, &main)
            .err()
            .unwrap_or_else(|| panic!("dist '{distrib}' should be rejected"));
        assert_eq!(
            err.code,
            ErrorCode::ConveyorSpreadflowUnsupported,
            "distrib '{distrib}'"
        );
    }
}

// ----- dist / source weight computation (§8), pure-function oracles -----

#[test]
fn dist_weights_array_indexes_by_floor_x_times_m() {
    // d=2 entry path: x_0 = 0.75 (exit slat), x_1 = 0.25 (entry slat).
    // floor(x*m) with m=2: floor(1.5)=1, floor(0.5)=0. So the exit slat reads
    // a[1] and the entry slat reads a[0].
    let profile = DistProfile::Array(vec![10.0, 0.0]);
    let w = dist_weights(&profile, 2);
    assert_eq!(w, vec![0.0, 10.0]);
}

#[test]
fn dist_weights_gf_samples_and_clamps_negatives() {
    // g(x) = 2x - 1 over [0,1]; d=2 -> x_0=0.75 -> 0.5, x_1=0.25 -> -0.5,
    // clamped to 0 by the max(0, .) rule (§8).
    let profile = DistProfile::Gf(vec![(0.0, -1.0), (1.0, 1.0)]);
    let w = dist_weights(&profile, 2);
    assert!((w[0] - 0.5).abs() < 1e-12, "exit weight {}", w[0]);
    assert_eq!(w[1], 0.0, "entry weight clamped to 0");
}

#[test]
fn dist_weights_empty_array_is_all_zero_fallback() {
    // All-zero weights make Placement::Dist fall back to Beginning.
    let w = dist_weights(&DistProfile::Array(vec![]), 3);
    assert_eq!(w, vec![0.0, 0.0, 0.0]);
}

#[test]
fn source_weights_equal_belts_mirror_positions() {
    // Equal belts (L_up = d = 4): each upstream slat maps to the same-index
    // target slat, so the mirror is the identity and conserves the total.
    let up = vec![1.0, 0.0, 0.0, 2.0];
    let w = source_weights(&up, 4);
    assert_eq!(w, vec![1.0, 0.0, 0.0, 2.0]);
    assert!((w.iter().sum::<f64>() - up.iter().sum::<f64>()).abs() < 1e-12);
}

#[test]
fn source_weights_different_belts_mirror_proportionally_ties_to_exit() {
    // L_up=2 -> y_0=0.75, y_1=0.25. d=4 -> x=[0.875,0.625,0.375,0.125].
    // y_0 ties between i=0 and i=1 (both 0.125 away) -> exit side i=0;
    // y_1 ties between i=2 and i=3 -> exit side i=2.
    let up = vec![3.0, 5.0];
    let w = source_weights(&up, 4);
    assert_eq!(w, vec![3.0, 0.0, 5.0, 0.0]);
    assert!((w.iter().sum::<f64>() - 8.0).abs() < 1e-12);
}

#[test]
fn source_weights_empty_inputs_are_all_zero() {
    assert_eq!(source_weights(&[], 3), vec![0.0, 0.0, 0.0]);
    assert_eq!(source_weights(&[1.0, 2.0], 0), Vec::<f64>::new());
}

/// The original full-scan `source_weights` (pre-GH #879), kept as the
/// explicit oracle for the windowed nearest-slat search: every upstream
/// slat volume lands at the target slat minimizing the float distance
/// `|x_i - y|`, first-wins (ties toward the exit) over an ascending scan
/// of ALL `d` slats.
fn source_weights_full_scan(up_slat_leak: &[f64], d: usize) -> Vec<f64> {
    let mut weights = vec![0.0; d];
    let l_up = up_slat_leak.len();
    if d == 0 || l_up == 0 {
        return weights;
    }
    for (j, &q) in up_slat_leak.iter().enumerate() {
        if q == 0.0 {
            continue;
        }
        let y = 1.0 - (j as f64 + 0.5) / l_up as f64;
        let mut best_i = 0usize;
        let mut best_dist = f64::INFINITY;
        for i in 0..d {
            let x_i = 1.0 - (i as f64 + 0.5) / d as f64;
            let dist = (x_i - y).abs();
            if dist < best_dist {
                best_dist = dist;
                best_i = i;
            }
        }
        weights[best_i] += q;
    }
    weights
}

#[test]
fn source_weights_windowed_search_matches_full_scan_across_geometries() {
    // The windowed nearest-slat search must be bit-for-bit the full scan
    // across diverse belt geometries: 1-slat belts, small primes,
    // exact-multiple ratios (which produce exact float ties, exercising
    // the first-wins tie rule), coprime near-tie ratios, and larger
    // prime/composite pairs. Every upstream slat carries a distinct
    // nonzero volume so each `j` exercises the window, and exact
    // `assert_eq!` on the vectors pins bit-identity (equal picked indices
    // imply identical accumulation order).
    let sizes: &[usize] = &[1, 2, 3, 4, 5, 6, 7, 8, 9, 12, 16, 25, 32, 48, 64, 100, 128];
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for &l_up in sizes {
        for &d in sizes {
            pairs.push((l_up, d));
        }
    }
    // Larger geometries: primes near 1000 (dense near-ties), a 1:2 exact
    // multiple, and strongly asymmetric belts in both directions.
    pairs.extend([(997, 1009), (1009, 997), (500, 1000), (3, 1009), (997, 4)]);

    for (l_up, d) in pairs {
        let up: Vec<f64> = (0..l_up).map(|j| 0.5 + j as f64 * 1.25).collect();
        assert_eq!(
            source_weights(&up, d),
            source_weights_full_scan(&up, d),
            "windowed search diverged from full scan at L_up={l_up}, d={d}"
        );
    }
}

// ----- compile-time spec advisories (§4.1 / §5.1 pure helpers) -----

#[test]
fn const_scalar_expr_accepts_literals_and_signs() {
    assert_eq!(const_scalar_expr("1.3"), Some(1.3));
    assert_eq!(const_scalar_expr(" 4 "), Some(4.0));
    assert_eq!(const_scalar_expr("-2.5"), Some(-2.5));
    assert_eq!(const_scalar_expr("+0.5"), Some(0.5));
    // Parentheses vanish at parse; the inner literal is still a constant.
    assert_eq!(const_scalar_expr("(1.5)"), Some(1.5));
}

#[test]
fn const_scalar_expr_rejects_runtime_expressions() {
    // A variable reference, arithmetic, a builtin, an empty equation, and
    // a parse error are all runtime/unknowable -- no compile-time value,
    // so the advisories that consume this must stay silent (§4.1/§5.1).
    assert_eq!(const_scalar_expr("transit_var"), None);
    assert_eq!(const_scalar_expr("1 + 2"), None);
    assert_eq!(const_scalar_expr("TIME"), None);
    assert_eq!(const_scalar_expr("MAX(1, 2)"), None);
    assert_eq!(const_scalar_expr(""), None);
    assert_eq!(const_scalar_expr("1 +"), None);
}

#[test]
fn transit_dt_mismatch_flags_non_multiples() {
    // The §4.1 example: 1.3 / 0.25 = 5.2 rounds half-away to 5 slats, an
    // effective transit of 1.25.
    assert_eq!(transit_dt_mismatch(1.3, 0.25), Some((5, 1.25)));
    // Half-away rounding mirror of slat_count: 1.5 / 1 rounds UP to 2.
    assert_eq!(transit_dt_mismatch(1.5, 1.0), Some((2, 2.0)));
    // A sub-dt transit clamps to one slat (slat_count's >= 1 clamp), so
    // the effective transit is a full dt.
    assert_eq!(transit_dt_mismatch(0.1, 0.25), Some((1, 0.25)));
}

#[test]
fn transit_dt_mismatch_exact_multiples_are_clean() {
    assert_eq!(transit_dt_mismatch(4.0, 0.25), None);
    assert_eq!(transit_dt_mismatch(1.0, 1.0), None);
    // Binary-representation noise stays below the 1e-9 ratio tolerance:
    // 0.3 / 0.1 is 2.9999999999999996 in f64 but an exact multiple in
    // intent, and must not warn.
    assert_eq!(transit_dt_mismatch(0.3, 0.1), None);
}

#[test]
fn leak_fraction_source_resolves_the_two_encodings_in_runtime_order() {
    let leak = |fraction: Option<&str>| datamodel::Leakage {
        fraction: fraction.map(str::to_string),
        integers: false,
        zone_start: None,
        zone_end: None,
    };
    let scalar = |s: &str| Equation::Scalar(s.to_string());

    // A non-empty explicit `<leak>` fraction wins over the flow's <eqn>.
    assert_eq!(
        leak_fraction_source(Some(&leak(Some("0.7"))), &scalar("0.5")),
        LeakFractionSource::Explicit("0.7")
    );
    // A bare `<leak/>` marker (fraction None) falls back to the flow's
    // own <eqn> -- the encoding real Stella files use (§3.3).
    let eqn = scalar("0.5");
    assert_eq!(
        leak_fraction_source(Some(&leak(None)), &eqn),
        LeakFractionSource::FlowEquation(&eqn)
    );
    // An EMPTY explicit fraction is treated like an absent one.
    assert_eq!(
        leak_fraction_source(Some(&leak(Some(""))), &eqn),
        LeakFractionSource::FlowEquation(&eqn)
    );
    // A truly bare marker on an empty-equation flow: no fraction at all.
    assert_eq!(
        leak_fraction_source(Some(&leak(None)), &scalar("")),
        LeakFractionSource::Absent
    );
    // A per-element equation always carries fractions.
    let arrayed = Equation::Arrayed(vec!["d1".to_string()], vec![], None, false);
    assert_eq!(
        leak_fraction_source(Some(&leak(None)), &arrayed),
        LeakFractionSource::FlowEquation(&arrayed)
    );
}

#[test]
fn transit_dt_mismatch_out_of_domain_inputs_are_other_diagnostics_jobs() {
    // Non-positive / non-finite transit is ConveyorTransitNotPositive's
    // domain at the runtime latch; an invalid dt is a sim-specs problem.
    assert_eq!(transit_dt_mismatch(0.0, 0.25), None);
    assert_eq!(transit_dt_mismatch(-1.0, 0.25), None);
    assert_eq!(transit_dt_mismatch(f64::INFINITY, 0.25), None);
    assert_eq!(transit_dt_mismatch(f64::NAN, 0.25), None);
    assert_eq!(transit_dt_mismatch(4.0, 0.0), None);
    assert_eq!(transit_dt_mismatch(4.0, -0.25), None);
    assert_eq!(transit_dt_mismatch(4.0, f64::NAN), None);
}

// ----- end-to-end dist / source placement -----

#[test]
fn dist_placement_end_to_end_sends_exit_weighted_inflow_out_early() {
    // profile g(x) = x concentrates weight near the exit (x -> 1), so some
    // admitted material lands close to the exit and leaves within a few DTs,
    // unlike the default `beginning` (0 outflow until the belt fills at
    // transit=4). Steady outflow still equals the inflow (conservation,
    // independent of placement).
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f" isee:spreadflow="dist"><eqn>250</eqn><isee:distrib_eq>profile</isee:distrib_eq></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <aux name="profile"><eqn>0+0</eqn><gf><xscale min="0" max="1"/><yscale min="0" max="1"/><ypts>0,0.25,0.5,0.75,1</ypts></gf></aux>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build dist");
    vm.run_to_end().expect("run");
    let out = vm.get_series(&Ident::new("out_f")).expect("out_f");
    let belt = vm.get_series(&Ident::new("belt")).expect("belt");
    assert!(
        out[2] > 0.0,
        "dist(exit-weighted): material should exit early, out[2]={}",
        out[2]
    );
    for (i, &b) in belt.iter().enumerate() {
        assert!(b.is_finite() && b >= -1e-9, "step {i}: belt {b}");
    }
    assert!(
        (out[out.len() - 1] - 250.0).abs() < 1e-4,
        "steady outflow {}",
        out[out.len() - 1]
    );
}

#[test]
fn source_placement_end_to_end_mirrors_upstream_leak() {
    // `leaking` is a linear leak of the upstream conveyor AND the inflow of
    // the downstream conveyor: the downstream admits it (conveyor-driven,
    // never blocked). At steady state the upstream leaks 0.2*250=50/time, so
    // whatever the placement the downstream (transit 4, no leak) settles to
    // outflow 50 (conservation). The placement geometry is what differs:
    // `beginning` deposits every leaked unit at the entry, so it traverses
    // the full transit and contents settle to 50*4=200; `source` mirrors the
    // upstream leak's slat positions, so material enters SHALLOWER than the
    // entry and exits sooner -- strictly lower steady contents.
    let model = |spread: &str| {
        wrap_model(&format!(
            r#"
    <stock name="up"><eqn>1000</eqn><inflow>src_in</inflow><outflow>up_out</outflow><outflow>leaking</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="src_in"><eqn>250</eqn></flow>
    <flow name="up_out"><eqn>0</eqn></flow>
    <flow name="leaking"{spread}><eqn>0.2</eqn><leak/></flow>
    <stock name="down"><eqn>0</eqn><inflow>leaking</inflow><outflow>down_out</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="down_out"><eqn>0</eqn></flow>"#
        ))
    };
    let run = |xml: &str| -> (f64, f64) {
        let project = parse(xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build");
        vm.run_to_end().expect("run");
        let down = vm.get_series(&Ident::new("down")).expect("down");
        let down_out = vm.get_series(&Ident::new("down_out")).expect("down_out");
        for (i, &b) in down.iter().enumerate() {
            assert!(b.is_finite() && b >= -1e-9, "step {i}: down {b}");
        }
        (down[down.len() - 1], down_out[down_out.len() - 1])
    };

    let (begin_contents, begin_out) = run(&model(""));
    let (source_contents, source_out) = run(&model(r#" isee:spreadflow="source""#));

    // Both conserve to the 50/time leak inflow.
    assert!(
        (begin_out - 50.0).abs() < 1e-3 && (source_out - 50.0).abs() < 1e-3,
        "steady outflows begin={begin_out} source={source_out} (want 50)"
    );
    // beginning fills the full transit to 200; source enters shallower.
    assert!(
        (begin_contents - 200.0).abs() < 1e-2,
        "beginning steady contents {begin_contents} (want 200)"
    );
    assert!(
        source_contents > 0.0 && source_contents < begin_contents - 1.0,
        "source contents {source_contents} should be positive and strictly \
         below beginning's {begin_contents}"
    );
}

#[test]
fn leak_into_arrested_conveyor_is_skipped_no_stock_belt_divergence() {
    // F6 regression (§4.3 step 2): conveyor `up` leaks flow `leaking` into
    // conveyor `down` (`<inflow>leaking</inflow>` on down). While `down` is
    // arrested the leak into it must be SKIPPED entirely (rate 0, `up` keeps
    // the material). If it is not, `up` keeps leaking, the ordinary Stocks
    // phase adds that rate to `down`'s stock slot, but `down`'s frozen belt
    // never admits it -- so the reported stock permanently climbs above the
    // true belt content (SUM(down)).
    let model = |arrest: &str| {
        wrap_model(&format!(
            r#"
    <stock name="up"><eqn>1000</eqn><inflow>src_in</inflow><outflow>up_out</outflow><outflow>leaking</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="src_in"><eqn>250</eqn></flow>
    <flow name="up_out"><eqn>0</eqn></flow>
    <flow name="leaking"><eqn>0.2</eqn><leak/></flow>
    <stock name="down"><eqn>0</eqn><inflow>leaking</inflow><outflow>down_out</outflow>
      <conveyor><len>4</len>{arrest}</conveyor></stock>
    <flow name="down_out"><eqn>0</eqn></flow>
    <aux name="down_belt"><eqn>SUM(down)</eqn></aux>
    <aux name="up_belt"><eqn>SUM(up)</eqn></aux>"#
        ))
    };
    let run = |xml: &str| -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>) {
        let project = parse(xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build");
        vm.run_to_end().expect("run");
        (
            vm.get_series(&Ident::new("down")).expect("down"),
            vm.get_series(&Ident::new("down_belt")).expect("down_belt"),
            vm.get_series(&Ident::new("leaking")).expect("leaking"),
            vm.get_series(&Ident::new("up_belt")).expect("up_belt"),
        )
    };

    // Arrest `down` for t in [5, 8): STEP(1,5) - STEP(1,8) == 1 over that
    // window. dt = 0.25, so those are steps [20, 32).
    let (down, down_belt, leaking, up_belt) =
        run(&model(r#"<arrest>STEP(1, 5) - STEP(1, 8)</arrest>"#));
    // Baseline: `down` never arrested (so `up` leaks the whole run).
    let (_bd, _bdb, _bl, base_up_belt) = run(&model(""));

    // The invariant the bug breaks: a conveyor's reported stock equals its
    // true belt content at EVERY step (conservation). The leak-into-arrested
    // bug makes `down` climb above SUM(down) during and after the arrest.
    for (i, (&s, &b)) in down.iter().zip(down_belt.iter()).enumerate() {
        assert!(
            (s - b).abs() < 1e-6,
            "step {i}: down stock {s} diverged from belt {b}"
        );
    }
    // During arrest the leak into `down` is skipped entirely (rate 0), so `up`
    // does not shed it (the material stays on up's belt to advance normally).
    // `i` is the semantic step index (arrest window t in [5, 8) == steps 20..32).
    #[allow(clippy::needless_range_loop)]
    for i in 20..32 {
        assert!(
            leaking[i].abs() < 1e-9,
            "step {i} (t={}): leaking={} should be 0 while down arrested",
            i as f64 * 0.25,
            leaking[i]
        );
    }
    // ... and resumes once `down` is released (t = 10, step 40).
    assert!(
        leaking[40] > 10.0,
        "step 40: leaking={} should resume after release",
        leaking[40]
    );
    // `up` retains the material it did NOT shed: at the last arrested step its
    // belt holds strictly more than the never-arrested baseline.
    assert!(
        up_belt[31] > base_up_belt[31] + 1.0,
        "up should retain un-leaked material: arrest {} vs baseline {}",
        up_belt[31],
        base_up_belt[31]
    );
}

#[test]
fn leak_dest_conveyor_meta_mirrors_primary_linkage() {
    // The leak's destination-conveyor linkage is resolved exactly like the
    // primary's (§4.3 step 2): a leak feeding a downstream conveyor records
    // that conveyor's stock name (so the runtime can skip it while the
    // destination is arrested); a leak to a cloud records None.
    let xml = wrap_model(
        r#"
    <stock name="up"><eqn>0</eqn><inflow>src_in</inflow><outflow>up_out</outflow><outflow>up_leak</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="src_in"><eqn>250</eqn></flow>
    <flow name="up_out"><eqn>0</eqn></flow>
    <flow name="up_leak"><eqn>0.2</eqn><leak/></flow>
    <stock name="down"><eqn>0</eqn><inflow>up_leak</inflow><outflow>down_out</outflow><outflow>down_drain</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="down_out"><eqn>0</eqn></flow>
    <flow name="down_drain"><eqn>0.1</eqn><leak/></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let (_expanded, metas) = expand_conveyors(&project, &main).expect("expand");
    let up = metas.iter().find(|m| m.stock == "up").expect("up meta");
    let down = metas.iter().find(|m| m.stock == "down").expect("down meta");
    // up's leak feeds `down` (a conveyor) -> recorded.
    assert_eq!(up.leaks.len(), 1);
    assert_eq!(up.leaks[0].dest_conveyor.as_deref(), Some("down"));
    // down's leak feeds a cloud (no conveyor lists it as an inflow) -> None.
    assert_eq!(down.leaks.len(), 1);
    assert_eq!(down.leaks[0].dest_conveyor, None);
}

#[test]
fn self_leak_flow_is_not_its_own_arrested_dest() {
    // A leak flow that also feeds its OWN conveyor records no destination (the
    // same `owner != stock` self-loop filter the primary uses): an arrested
    // conveyor never leaks at all, so a self-leak can never hold against its
    // own arrest.
    let xml = wrap_model(
        r#"
    <stock name="a"><eqn>0</eqn><inflow>a_in</inflow><inflow>a_leak</inflow><outflow>a_out</outflow><outflow>a_leak</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="a_in"><eqn>250</eqn></flow>
    <flow name="a_out"><eqn>0</eqn></flow>
    <flow name="a_leak"><eqn>0.1</eqn><leak/></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let (_expanded, metas) = expand_conveyors(&project, &main).expect("expand");
    let a = metas.iter().find(|m| m.stock == "a").expect("a meta");
    assert_eq!(a.leaks.len(), 1);
    assert_eq!(
        a.leaks[0].dest_conveyor, None,
        "a self-leak must not link to its own conveyor"
    );
}

#[test]
fn leak_to_cloud_keeps_flowing_while_another_conveyor_is_arrested() {
    // No false positive: a leak to a cloud (no downstream conveyor) must NOT
    // be skipped just because some OTHER conveyor in the model is arrested --
    // the skip is keyed on the leak's OWN destination (§4.3 step 2).
    let xml = wrap_model(
        r#"
    <stock name="up"><eqn>1000</eqn><inflow>up_in</inflow><outflow>up_out</outflow><outflow>up_leak</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="up_in"><eqn>250</eqn></flow>
    <flow name="up_out"><eqn>0</eqn></flow>
    <flow name="up_leak"><eqn>0.2</eqn><leak/></flow>
    <stock name="other"><eqn>1000</eqn><inflow>other_in</inflow><outflow>other_out</outflow>
      <conveyor><len>4</len><arrest>STEP(1, 5) - STEP(1, 8)</arrest></conveyor></stock>
    <flow name="other_in"><eqn>250</eqn></flow>
    <flow name="other_out"><eqn>0</eqn></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build");
    vm.run_to_end().expect("run");
    let up_leak = vm.get_series(&Ident::new("up_leak")).expect("up_leak");
    // `other` is arrested for steps 20..32; `up_leak` goes to a cloud, so it
    // keeps flowing throughout. `i` is the semantic step index.
    #[allow(clippy::needless_range_loop)]
    for i in 20..32 {
        assert!(
            up_leak[i] > 10.0,
            "step {i}: up_leak={} should keep flowing (cloud dest, not arrested)",
            up_leak[i]
        );
    }
}

#[test]
fn source_on_non_leak_inflow_falls_back_to_beginning() {
    // `source` on an ordinary equation-driven inflow (no upstream leak to
    // mirror) must not error: it degrades to `beginning`, so the belt fills
    // over the transit exactly like the default -- 0 outflow for 16 steps.
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f" isee:spreadflow="source"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build source-fallback");
    vm.run_to_end().expect("run");
    let out = vm.get_series(&Ident::new("out_f")).expect("out_f");
    for (i, &g) in out.iter().enumerate().take(16) {
        assert!(g.abs() < 1e-9, "step {i}: fallback outflow {g} should be 0");
    }
    assert!((out[20] - 250.0).abs() < 1e-4, "steady outflow {}", out[20]);
}

// ----- arrayed conveyors (§10) -----

#[test]
fn arrayed_conveyor_simulates_independent_belts() {
    // An arrayed conveyor is N_elem independent belts (§10). `board` has two
    // elements with DIFFERENT transit times (a=2, b=4, via the shared <len>
    // referencing the arrayed `transit` aux) and DIFFERENT inflows (a=100,
    // b=250). Each element must reach its own steady state and its own
    // transit delay, proving the belts are independent.
    //   belt[a]: transit 2 -> 8 slats, inflow 100 -> steady 200, out=100.
    //   belt[b]: transit 4 -> 16 slats, inflow 250 -> steady 1000, out=250.
    let xml = include_str!("../../../test/conveyors/arrayed_conveyor.xmile");
    let project = parse(xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build arrayed conveyor vm");
    vm.run_to_end().expect("run");

    let belt_a = vm.get_series(&Ident::new("belt[a]")).expect("belt[a]");
    let belt_b = vm.get_series(&Ident::new("belt[b]")).expect("belt[b]");
    let out_a = vm
        .get_series(&Ident::new("outflow_f[a]"))
        .expect("outflow_f[a]");
    let out_b = vm
        .get_series(&Ident::new("outflow_f[b]"))
        .expect("outflow_f[b]");
    assert!(belt_a.len() > 40, "should have many saved steps");

    // Independent transit delays: dt=0.25. belt[a] (8 slats) exits the first
    // cohort at t=2 (step 8); belt[b] (16 slats) not until t=4 (step 16).
    for (i, &g) in out_a.iter().enumerate().take(8) {
        assert!(g.abs() < 1e-9, "belt[a] step {i}: outflow {g} should be 0");
    }
    for (i, &g) in out_b.iter().enumerate().take(16) {
        assert!(g.abs() < 1e-9, "belt[b] step {i}: outflow {g} should be 0");
    }
    // belt[a] is already full/steady at step 8, but belt[b] (transit 4) is not
    // yet -- so at step 8 the two belts are in DIFFERENT states, which is the
    // whole point of independence.
    assert!(
        (out_a[8] - 100.0).abs() < 1e-6,
        "belt[a] outflow at step 8 {} (want 100)",
        out_a[8]
    );
    assert!(
        out_b[8].abs() < 1e-9,
        "belt[b] still filling at step 8, outflow {} (want 0)",
        out_b[8]
    );

    // Independent steady states.
    let last = belt_a.len() - 1;
    assert!(
        (belt_a[last] - 200.0).abs() < 1e-4,
        "belt[a] steady contents {} (want 200)",
        belt_a[last]
    );
    assert!(
        (belt_b[last] - 1000.0).abs() < 1e-4,
        "belt[b] steady contents {} (want 1000)",
        belt_b[last]
    );
    assert!(
        (out_a[last] - 100.0).abs() < 1e-6 && (out_b[last] - 250.0).abs() < 1e-6,
        "steady outflows a={} b={} (want 100, 250)",
        out_a[last],
        out_b[last]
    );
}

#[test]
fn arrayed_conveyor_expands_to_one_plan_per_element() {
    // resolve_plans flattens an arrayed conveyor into one plan per element,
    // each pointing at that element's contiguous data-buffer slots -- and the
    // per-element stock/len/flow slots must be DISTINCT (independent belts).
    let xml = include_str!("../../../test/conveyors/arrayed_conveyor.xmile");
    let project = parse(xml);
    let main = project.models[0].name.clone();
    let (compiled, plans, queue_plans) =
        build_compiled_fresh(&project, &main).expect("build_compiled");
    assert!(
        queue_plans.is_empty(),
        "a pure-conveyor model must synthesize zero queue plans"
    );
    assert_eq!(plans.len(), 2, "two elements -> two flattened plans");
    assert_ne!(
        plans[0].stock_off, plans[1].stock_off,
        "each element's belt reads a distinct stock slot"
    );
    assert_ne!(
        plans[0].len_off, plans[1].len_off,
        "each element has its own transit-time slot"
    );
    assert_ne!(
        plans[0].primary_out_off, plans[1].primary_out_off,
        "each element writes a distinct outflow slot"
    );
    // Sanity: the resolved offsets are real slots in the compiled buffer.
    assert_eq!(
        compiled.get_offset(&Ident::new("belt[a]")),
        Some(plans[0].stock_off)
    );
    assert_eq!(
        compiled.get_offset(&Ident::new("belt[b]")),
        Some(plans[1].stock_off)
    );
}

#[test]
fn arrayed_conveyor_with_shared_leak_conserves_per_element() {
    // A shared linear leak fraction (0.2) applied apply-to-all across both
    // elements. Each belt conserves independently: steady outflow = inflow *
    // (1 - 0.2) = 0.8 * inflow, leak = 0.2 * inflow, per element.
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>attriting</outflow>
      <dimensions><dim name="board"/></dimensions>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f">
      <element subscript="a"><eqn>100</eqn></element>
      <element subscript="b"><eqn>250</eqn></element>
      <dimensions><dim name="board"/></dimensions></flow>
    <flow name="out_f"><dimensions><dim name="board"/></dimensions></flow>
    <flow name="attriting"><eqn>0.2</eqn><leak/><dimensions><dim name="board"/></dimensions></flow>"#,
    );
    // wrap_model has no <dimensions>; inject the board dimension.
    let xml = xml.replace(
        "<model>",
        "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build arrayed leak vm");
    vm.run_to_end().expect("run");

    let out_a = vm.get_series(&Ident::new("out_f[a]")).expect("out_f[a]");
    let out_b = vm.get_series(&Ident::new("out_f[b]")).expect("out_f[b]");
    let leak_a = vm
        .get_series(&Ident::new("attriting[a]"))
        .expect("attriting[a]");
    let leak_b = vm
        .get_series(&Ident::new("attriting[b]"))
        .expect("attriting[b]");
    let last = out_a.len() - 1;
    assert!(
        (out_a[last] - 80.0).abs() < 1e-3,
        "belt[a] steady outflow {} (want 80)",
        out_a[last]
    );
    assert!(
        (out_b[last] - 200.0).abs() < 1e-3,
        "belt[b] steady outflow {} (want 200)",
        out_b[last]
    );
    assert!(
        (leak_a[last] - 20.0).abs() < 1e-3,
        "belt[a] steady leak {} (want 20)",
        leak_a[last]
    );
    assert!(
        (leak_b[last] - 50.0).abs() < 1e-3,
        "belt[b] steady leak {} (want 50)",
        leak_b[last]
    );
}

#[test]
fn arrayed_leak_into_arrested_conveyor_skips_per_element() {
    // Element-wise wiring of the §4.3 step-2 skip: arrayed `up` leaks into
    // arrayed `down` element-for-element (leak[e] -> down[e]); arrest only
    // down[a] (a per-element arrest driver). During the window down[a]'s
    // inbound leak (leaking[a]) is skipped while down[b]'s (leaking[b]) keeps
    // flowing, and down[a]'s reported stock never diverges from its frozen
    // belt.
    let xml = wrap_model(
        r#"
    <stock name="up"><eqn>1000</eqn><inflow>src_in</inflow><outflow>up_out</outflow><outflow>leaking</outflow>
      <dimensions><dim name="board"/></dimensions>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="src_in"><eqn>250</eqn><dimensions><dim name="board"/></dimensions></flow>
    <flow name="up_out"><dimensions><dim name="board"/></dimensions></flow>
    <flow name="leaking"><eqn>0.2</eqn><leak/><dimensions><dim name="board"/></dimensions></flow>
    <stock name="down"><eqn>0</eqn><inflow>leaking</inflow><outflow>down_out</outflow>
      <dimensions><dim name="board"/></dimensions>
      <conveyor><len>4</len><arrest>arrest_drv</arrest></conveyor></stock>
    <flow name="down_out"><dimensions><dim name="board"/></dimensions></flow>
    <aux name="arrest_drv">
      <element subscript="a"><eqn>STEP(1, 5) - STEP(1, 8)</eqn></element>
      <element subscript="b"><eqn>0</eqn></element>
      <dimensions><dim name="board"/></dimensions></aux>
    <aux name="down_a_belt"><eqn>SUM(down[a])</eqn></aux>"#,
    );
    // wrap_model has no <dimensions>; inject the board dimension.
    let xml = xml.replace(
        "<model>",
        "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build arrayed arrest vm");
    vm.run_to_end().expect("run");
    let leak_a = vm
        .get_series(&Ident::new("leaking[a]"))
        .expect("leaking[a]");
    let leak_b = vm
        .get_series(&Ident::new("leaking[b]"))
        .expect("leaking[b]");
    let down_a = vm.get_series(&Ident::new("down[a]")).expect("down[a]");
    let down_a_belt = vm
        .get_series(&Ident::new("down_a_belt"))
        .expect("down_a_belt");
    // Element a's destination (down[a]) is arrested for steps 20..32: its
    // inbound leak is skipped; element b's is not.
    for i in 20..32 {
        assert!(
            leak_a[i].abs() < 1e-9,
            "step {i}: leaking[a]={} should be 0 (down[a] arrested)",
            leak_a[i]
        );
        assert!(
            leak_b[i] > 10.0,
            "step {i}: leaking[b]={} should keep flowing (down[b] not arrested)",
            leak_b[i]
        );
    }
    // down[a]'s reported stock never diverges from its frozen belt.
    for (i, (&s, &b)) in down_a.iter().zip(down_a_belt.iter()).enumerate() {
        assert!(
            (s - b).abs() < 1e-6,
            "step {i}: down[a] stock {s} diverged from belt {b}"
        );
    }
}

// ----- container access (§10): native computation + residual rejection -----

/// Build the standard scalar-conveyor model plus a `reader` aux with the
/// given equation, and return the `build_vm` result. The belt fills from
/// empty (init 0).
fn build_with_reader(reader_eqn: &str) -> crate::common::Result<crate::vm::Vm> {
    let xml = wrap_model(&format!(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <aux name="reader"><eqn>{reader_eqn}</eqn></aux>"#
    ));
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    build_vm(&project, &main)
}

/// Build a STEADY-STATE scalar-conveyor model (belt init 1000, inflow 250,
/// transit 4, dt 0.25 -> 16 slats each holding 62.5) plus a `reader` aux, run
/// it, and return `reader`'s series. Every hand-computed oracle in these
/// tests reads this known belt.
fn steady_reader_series(reader_eqn: &str) -> Vec<f64> {
    let xml = wrap_model(&format!(
        r#"
    <stock name="belt"><eqn>1000</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <aux name="reader"><eqn>{reader_eqn}</eqn></aux>"#
    ));
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build steady reader");
    vm.run_to_end().expect("run");
    vm.get_series(&Ident::new("reader")).expect("reader series")
}

#[test]
fn scalar_container_reducers_read_the_belt() {
    // The steady belt is 16 slats each 62.5 (SUM 1000). SUM/MEAN/SIZE/MIN/
    // MAX/STDDEV are now computed natively from the belt, not rejected (§10).
    let cases = [
        ("SUM(belt)", 1000.0),
        ("MEAN(belt)", 62.5),
        ("SIZE(belt)", 16.0),
        ("MIN(belt)", 62.5),
        ("MAX(belt)", 62.5),
        ("STDDEV(belt)", 0.0),
    ];
    for (eqn, want) in cases {
        let series = steady_reader_series(eqn);
        for (i, &v) in series.iter().enumerate() {
            assert!(
                (v - want).abs() < 1e-9,
                "'{eqn}' step {i}: got {v}, want {want}"
            );
        }
    }
}

#[test]
fn container_value_from_slice_pins_belt_conventions() {
    // The conveyor publish pass drives the SHARED container_value_from_slice
    // over ConveyorState::slat_contents(); this pins the per-kind numerics
    // it relies on. A physically EMPTY belt (zero slats) is unreachable
    // through belt init (init_steady always builds >= 1 slat), so the
    // empty-container conventions -- Sum -> 0 (additive identity), Size -> 0,
    // Mean/Min/Max/Stddev -> NaN, any Slat index -> NaN, matching the VM's
    // empty array reducers (`vm.rs`) -- are pinned here at the unit level.
    let empty_belt = ConveyorState::new(0.25, false, false, false, Vec::new());
    let slats = empty_belt.slat_contents();
    assert!(slats.is_empty(), "a fresh belt has zero slats");
    assert_eq!(container_value_from_slice(&slats, &ContainerKind::Sum), 0.0);
    assert_eq!(
        container_value_from_slice(&slats, &ContainerKind::Size),
        0.0
    );
    for kind in [
        ContainerKind::Mean,
        ContainerKind::Min,
        ContainerKind::Max,
        ContainerKind::Stddev,
        ContainerKind::Slat(1),
    ] {
        assert!(
            container_value_from_slice(&slats, &kind).is_nan(),
            "{kind:?} over an empty belt is NaN"
        );
    }
    // A non-uniform vector with hand-computed oracles: Slat(j) is 1-based
    // with both out-of-range sides NaN, and Stddev is the POPULATION
    // standard deviation (divisor N, not N-1).
    let v = [1.0, 2.0, 3.0, 4.0];
    assert_eq!(container_value_from_slice(&v, &ContainerKind::Slat(1)), 1.0);
    assert_eq!(container_value_from_slice(&v, &ContainerKind::Slat(4)), 4.0);
    assert!(container_value_from_slice(&v, &ContainerKind::Slat(0)).is_nan());
    assert!(container_value_from_slice(&v, &ContainerKind::Slat(5)).is_nan());
    assert_eq!(container_value_from_slice(&v, &ContainerKind::Sum), 10.0);
    assert_eq!(container_value_from_slice(&v, &ContainerKind::Mean), 2.5);
    assert_eq!(container_value_from_slice(&v, &ContainerKind::Size), 4.0);
    assert_eq!(container_value_from_slice(&v, &ContainerKind::Min), 1.0);
    assert_eq!(container_value_from_slice(&v, &ContainerKind::Max), 4.0);
    // mean 2.5 -> squared deviations (2.25, 0.25, 0.25, 2.25) -> var 1.25.
    let stddev = container_value_from_slice(&v, &ContainerKind::Stddev);
    assert!(
        (stddev - 1.25f64.sqrt()).abs() < 1e-12,
        "population stddev sqrt(1.25), got {stddev}"
    );
}

#[test]
fn scalar_slat_index_reads_slat_and_out_of_range_is_nan() {
    // conv[j] is 1-based from the exit. On the 16-slat steady belt every slat
    // is 62.5; conv[0] and conv[17] are out of range -> NaN (§10).
    assert!((steady_reader_series("belt[1]")[10] - 62.5).abs() < 1e-9);
    assert!((steady_reader_series("belt[16]")[10] - 62.5).abs() < 1e-9);
    assert!(
        steady_reader_series("belt[0]")[10].is_nan(),
        "belt[0] -> NaN"
    );
    assert!(
        steady_reader_series("belt[17]")[10].is_nan(),
        "belt[17] -> NaN"
    );
}

#[test]
fn container_access_in_larger_expression_and_conditional() {
    // A supported container access nested inside a larger expression / an IF
    // is rewritten in place (not whole-equation), so the surrounding math is
    // preserved: SUM(belt) + 1 == 1001; IF belt[2] > 0 THEN 1 ELSE 0 == 1.
    assert!((steady_reader_series("SUM(belt) + 1")[5] - 1001.0).abs() < 1e-9);
    for &v in steady_reader_series("IF belt[2] > 0 THEN 1 ELSE 0").iter() {
        assert_eq!(v, 1.0);
    }
}

#[test]
fn container_init_reads_start_of_run_value_not_placeholder() {
    // INIT(<container access>) must read the belt's START-OF-RUN value, not
    // the hidden container stock's '0' <eqn> placeholder. The rewrite turns
    // both SUM(belt) and INIT(SUM(belt)) into the hidden stock $conv$sum$belt;
    // its initial_values snapshot must be patched to the initialized belt's
    // total. On the steady belt SUM(belt)==1000 every step, so the ratio is
    // 1.0; before the fix INIT read the frozen 0 and the ratio was +inf.
    let ratio = steady_reader_series("SUM(belt) / INIT(SUM(belt))");
    for (i, &v) in ratio.iter().enumerate() {
        assert!(
            (v - 1.0).abs() < 1e-9,
            "step {i}: SUM(belt)/INIT(SUM(belt)) = {v} (want 1.0; pre-fix +inf)"
        );
    }
    // INIT of a reducer, SIZE, and a slat index all read the start-of-run
    // belt (16 slats of 62.5): pre-fix every one read the frozen 0.
    for (eqn, want) in [
        ("INIT(SUM(belt))", 1000.0),
        ("INIT(SIZE(belt))", 16.0),
        ("INIT(belt[1])", 62.5),
    ] {
        let series = steady_reader_series(eqn);
        for (i, &v) in series.iter().enumerate() {
            assert!(
                (v - want).abs() < 1e-9,
                "'{eqn}' step {i}: got {v}, want {want} (pre-fix 0)"
            );
        }
    }
}

#[test]
fn stock_initialized_from_container_access_reads_start_of_run() {
    // A plain stock whose <eqn> is a container access (no INIT wrapper) must
    // also start from the belt's start-of-run total: the belt/queue init runs
    // after the initials snapshot, so the initials pass first sees the '0'
    // placeholder, and the reconciliation re-run recomputes the stock's initial
    // value from the published belt. `accum` starts at SUM(belt)=1000 and, with
    // no flows, holds flat -- pre-fix it started at 0.
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>1000</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <stock name="accum"><eqn>SUM(belt)</eqn></stock>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build stock-init vm");
    vm.run_to_end().expect("run");
    let accum = vm.get_series(&Ident::new("accum")).expect("accum");
    assert!(
        (accum[0] - 1000.0).abs() < 1e-9,
        "accum[0] = {} (want 1000; pre-fix 0)",
        accum[0]
    );
    for (i, &v) in accum.iter().enumerate() {
        assert!(
            (v - 1000.0).abs() < 1e-9,
            "step {i}: accum = {v} (a no-flow stock initialized to 1000 stays flat)"
        );
    }
}

#[test]
fn container_init_survives_reset_and_rerun() {
    // libsimlin's reset recreates the belt side table and re-runs
    // run_initials, so the container-value reconciliation must be idempotent:
    // INIT(SUM(belt)) must still read the start-of-run 1000 after a reset,
    // not accumulate or drift. The re-run derives only from freshly published
    // belt state, so it is idempotent by construction.
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>1000</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <aux name="reader"><eqn>INIT(SUM(belt))</eqn></aux>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build reset vm");
    vm.run_to_end().expect("first run");
    let first = vm.get_series(&Ident::new("reader")).expect("reader");
    vm.reset();
    vm.run_to_end().expect("second run");
    let second = vm.get_series(&Ident::new("reader")).expect("reader");
    assert_eq!(first, second, "reset+rerun must reproduce INIT(SUM(belt))");
    for (i, &v) in second.iter().enumerate() {
        assert!(
            (v - 1000.0).abs() < 1e-9,
            "step {i}: INIT(SUM(belt)) = {v} after reset (want 1000)"
        );
    }
}

#[test]
fn arrayed_container_init_patches_every_element_slot() {
    // An arrayed conveyor's container stock is arrayed over the owner's dims,
    // so the initial_values patch-up must reach EVERY element slot, not just
    // the first. Both belts start at steady 1000 (inflow 250, transit 4), so
    // SUM(belt[a])==SUM(belt[b])==1000 every step and INIT(SUM(belt[a]))==1000
    // for each element (pre-fix 0 -> ratio +inf for both).
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>1000</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <dimensions><dim name="board"/></dimensions>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn><dimensions><dim name="board"/></dimensions></flow>
    <flow name="out_f"><dimensions><dim name="board"/></dimensions></flow>
    <aux name="ratio_a"><eqn>SUM(belt[a]) / INIT(SUM(belt[a]))</eqn></aux>
    <aux name="init_b"><eqn>INIT(SUM(belt[b]))</eqn></aux>"#,
    );
    // wrap_model has no <dimensions>; inject the board dimension.
    let xml = xml.replace(
        "<model>",
        "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build arrayed init vm");
    vm.run_to_end().expect("run");
    let ratio_a = vm.get_series(&Ident::new("ratio_a")).expect("ratio_a");
    for (i, &v) in ratio_a.iter().enumerate() {
        assert!(
            (v - 1.0).abs() < 1e-9,
            "step {i}: SUM(belt[a])/INIT(SUM(belt[a])) = {v} (want 1.0; pre-fix +inf)"
        );
    }
    let init_b = vm.get_series(&Ident::new("init_b")).expect("init_b");
    for (i, &v) in init_b.iter().enumerate() {
        assert!(
            (v - 1000.0).abs() < 1e-9,
            "step {i}: INIT(SUM(belt[b])) = {v} (want 1000; pre-fix 0 -- second element slot)"
        );
    }
}

#[test]
fn container_reducer_on_filling_belt_tracks_min_max_stddev() {
    // A belt filling from empty exercises MIN/MAX/STDDEV over a non-uniform
    // and briefly-empty belt. Insert-at-entry (default `beginning`) means an
    // empty belt after step 0's insert holds one slat; MIN of a partially
    // filled belt (some 0 slats) is 0 while MAX rises toward the inflow
    // cohort (250*0.25 = 62.5), and STDDEV is > 0 mid-fill.
    let min_s = steady_reader_min_max_stddev("MIN(belt)");
    let max_s = steady_reader_min_max_stddev("MAX(belt)");
    let std_s = steady_reader_min_max_stddev("STDDEV(belt)");
    // Early in the fill the belt has both 0 slats and a 62.5 cohort.
    assert_eq!(min_s[2], 0.0, "MIN over a partly-empty belt is 0");
    assert!(
        (max_s[2] - 62.5).abs() < 1e-9,
        "MAX over the cohort is 62.5, got {}",
        max_s[2]
    );
    assert!(
        std_s[2] > 0.0,
        "STDDEV mid-fill is positive, got {}",
        std_s[2]
    );
    // Once full and steady every slat is 62.5: MIN==MAX, STDDEV==0.
    let last = min_s.len() - 1;
    assert!((min_s[last] - 62.5).abs() < 1e-9 && (max_s[last] - 62.5).abs() < 1e-9);
    assert!(
        std_s[last].abs() < 1e-9,
        "steady STDDEV 0, got {}",
        std_s[last]
    );
}

/// A filling-from-empty belt reader series (belt init 0, inflow 250).
fn steady_reader_min_max_stddev(reader_eqn: &str) -> Vec<f64> {
    let mut vm = build_with_reader(reader_eqn).expect("build filling reader");
    vm.run_to_end().expect("run");
    vm.get_series(&Ident::new("reader")).expect("reader")
}

#[test]
fn container_value_is_start_of_step_and_feeds_a_flow_same_step() {
    // The CRUX (§10): a container value is read DURING the flows phase, must
    // reflect START-OF-STEP belt state, and must survive the flows eval so it
    // is not clobbered. `reader = SUM(belt)` feeds both another aux
    // (`doubled = reader * 2`) and a flow into a sink stock (`accum`), all in
    // the same step. On the steady belt SUM(belt) == 1000 every step, so
    // doubled == 2000 and accum accumulates 1000/time.
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>1000</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <aux name="reader"><eqn>SUM(belt)</eqn></aux>
    <aux name="doubled"><eqn>reader * 2</eqn></aux>
    <stock name="accum"><eqn>0</eqn><inflow>sink_f</inflow></stock>
    <flow name="sink_f"><eqn>reader</eqn></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build crux model");
    vm.run_to_end().expect("run");
    let reader = vm.get_series(&Ident::new("reader")).expect("reader");
    let doubled = vm.get_series(&Ident::new("doubled")).expect("doubled");
    let accum = vm.get_series(&Ident::new("accum")).expect("accum");
    for (i, (&r, &d)) in reader.iter().zip(doubled.iter()).enumerate() {
        assert!((r - 1000.0).abs() < 1e-9, "step {i} reader {r}");
        assert!((d - 2000.0).abs() < 1e-9, "step {i} doubled {d}");
    }
    // accum integrates 1000/time from t=0; the final value ~= 1000 * stop.
    let last = accum[accum.len() - 1];
    assert!(
        (last - 1000.0 * 20.0).abs() < 1.0,
        "accum final {last} (want ~20000)"
    );
}

#[test]
fn container_reducer_is_read_before_this_steps_insert() {
    // Start-of-step timing (§10): a reader of SUM(belt) sees the belt BEFORE
    // this step's exit/insert. The belt fills from empty (16 zero-slats,
    // inflow 250 -> 62.5 inserted per step, nothing exits for 16 steps), so
    // SUM(belt) at step t reflects only the t inserts made in the PRIOR steps:
    // reader[t] == 62.5 * t for t <= 16, then plateaus at 1000. If the value
    // were recomputed after this step's insert it would read 62.5 higher.
    let series = steady_reader_min_max_stddev("SUM(belt)"); // belt init 0
    assert_eq!(series[0], 0.0, "step 0: no inserts yet");
    for (t, &v) in series.iter().enumerate().take(17).skip(1) {
        assert!(
            (v - 62.5 * t as f64).abs() < 1e-9,
            "step {t}: SUM(belt)={v} (want {}, start-of-step)",
            62.5 * t as f64
        );
    }
    assert!((series[20] - 1000.0).abs() < 1e-9, "plateaus at 1000");
}

#[test]
fn container_access_in_conveyor_parameter_expression_is_computed() {
    // A container access inside the conveyor's OWN parameter expression must
    // be rewritten too (not just ordinary equations): `<capacity>SIZE(belt)`
    // binds to the belt length (16), so contents plateau at 16 -- NOT the
    // silent SIZE(scalar)=1 that plateaued at 1 before the fix.
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>4</len><capacity>SIZE(belt)</capacity></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build param-container vm");
    vm.run_to_end().expect("run");
    let belt = vm.get_series(&Ident::new("belt")).expect("belt");
    for (i, &b) in belt.iter().enumerate() {
        assert!(
            b <= 16.0 + 1e-6,
            "step {i}: contents {b} exceeds capacity 16"
        );
    }
    let last = belt[belt.len() - 1];
    assert!(
        (last - 16.0).abs() < 1e-6,
        "capacity=SIZE(belt)=16 plateaus contents at 16, got {last} (silent-wrong would be 1)"
    );
}

#[test]
fn residual_container_access_in_conveyor_parameter_is_rejected() {
    // A residual container form in a parameter or leak-fraction expression is
    // loud-rejected exactly like one in an ordinary equation -- never
    // silently mis-bound to the scalar stock.
    let cases = [
        r#"<conveyor><len>4</len><capacity>MEAN(belt / 2)</capacity></conveyor>"#,
        r#"<conveyor><len>MEAN(belt / 2)</len></conveyor>"#,
    ];
    for conveyor in cases {
        let xml = wrap_model(&format!(
            r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      {conveyor}</stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#
        ));
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let err = build_vm(&project, &main)
            .err()
            .unwrap_or_else(|| panic!("residual param '{conveyor}' should be rejected"));
        assert_eq!(
            err.code,
            ErrorCode::ConveyorContainerAccessUnsupported,
            "conveyor '{conveyor}'"
        );
    }
}

#[test]
fn residual_container_access_in_leak_fraction_is_rejected() {
    // A residual container form in a leak-fraction expression is likewise
    // loud-rejected (the fraction is synthesized into a `$conv$leak$...$frac`
    // aux from its raw string, so it must be rewritten/checked too).
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>attriting</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <flow name="attriting"><eqn>MEAN(belt / 2)</eqn><leak/></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let err = build_vm(&project, &main)
        .expect_err("residual leak-fraction container access should be rejected");
    assert_eq!(err.code, ErrorCode::ConveyorContainerAccessUnsupported);
}

#[test]
fn scalar_dynamic_and_ranged_container_access_still_rejected() {
    // The genuinely-unlowerable residuals stay loud-rejected (§10): a dynamic
    // (non-constant) slat index and a slat range/wildcard need the per-slat
    // vector, which cannot be reduced to one native scalar.
    for eqn in ["belt[k]", "belt[1:2]", "belt[*]"] {
        let err = build_with_reader(&format!("{eqn} + 0"))
            .err()
            .unwrap_or_else(|| panic!("residual '{eqn}' should be rejected"));
        assert_eq!(
            err.code,
            ErrorCode::ConveyorContainerAccessUnsupported,
            "equation '{eqn}'"
        );
    }
}

#[test]
fn scalar_wrapped_reducer_forms_are_rejected() {
    // Finding 2: a scalar conveyor wrapped in ANY expression inside a
    // single-arg reducer still means the belt's slats (why else reduce a
    // scalar?), so it must be rejected -- not silently return the belt total.
    for eqn in [
        "MEAN(belt + 0)",
        "MEAN(belt / 2)",
        "MIN(belt * 2)",
        "SUM(belt - 1)",
        "STDDEV(2 * belt + 3)",
        "SIZE(belt + belt)",
    ] {
        let err = build_with_reader(eqn)
            .err()
            .unwrap_or_else(|| panic!("wrapped reducer '{eqn}' should be rejected"));
        assert_eq!(
            err.code,
            ErrorCode::ConveyorContainerAccessUnsupported,
            "equation '{eqn}'"
        );
    }
}

#[test]
fn scalar_min_max_with_two_args_reduces_belt_total_not_belt() {
    // MIN/MAX with a second argument is scalar min/max of the belt TOTAL, not
    // belt-container access -- it must NOT be rejected and simulates fine.
    for eqn in ["MIN(belt, 5)", "MAX(belt, 5)"] {
        let mut vm = build_with_reader(eqn)
            .unwrap_or_else(|_| panic!("scalar min/max of belt total '{eqn}' should compile"));
        vm.run_to_end().expect("run");
    }
}

#[test]
fn non_container_conveyor_reads_are_unaffected() {
    // A bare read of a conveyor's scalar value (its belt total) is ordinary
    // and must NOT be flagged as container access.
    let mut vm = build_with_reader("belt * 2").expect("bare read of belt total is fine");
    vm.run_to_end().expect("run");
}

/// Build the standard arrayed-conveyor model (board {a,b}; inflow a=100,
/// b=250; transit 4; belt filling from empty) plus a `reader` aux, returning
/// the `build_vm` result.
fn build_arrayed_reader(reader: &str) -> crate::common::Result<crate::vm::Vm> {
    let xml = wrap_model(&format!(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <dimensions><dim name="board"/></dimensions>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f">
      <element subscript="a"><eqn>100</eqn></element>
      <element subscript="b"><eqn>250</eqn></element>
      <dimensions><dim name="board"/></dimensions></flow>
    <flow name="out_f"><dimensions><dim name="board"/></dimensions></flow>
    <aux name="reader"><eqn>{reader}</eqn></aux>"#
    ))
    .replace(
        "<model>",
        "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    build_vm(&project, &main)
}

#[test]
fn arrayed_conveyor_ordinary_array_reads_allowed() {
    // For an arrayed conveyor, reading one element (`belt[a]`, that belt's
    // TOTAL) and reducing over the per-element totals (`SUM(belt)`) are
    // ordinary array ops -- unchanged, no container synthesis.
    for eqn in ["belt[a]", "SUM(belt)", "SUM(belt[*])", "SUM(belt * 2)"] {
        let mut vm = build_arrayed_reader(eqn)
            .unwrap_or_else(|_| panic!("ordinary array read '{eqn}' should compile"));
        vm.run_to_end().expect("run");
    }
}

#[test]
fn arrayed_single_belt_container_access_computes_per_element() {
    // A single-belt subscript reduced by a reducer, or a per-element belt
    // slot, is now supported (§10): the container variable is arrayed over
    // the conveyor's dims, so `SUM(belt[a])` reads belt a and `belt[b, 2]`
    // reads belt b's slat 2. Steady: belt[a] = 16 slats of 25 (SUM 400),
    // belt[b] = 16 slats of 62.5 (SUM 1000).
    let series = |reader: &str| -> Vec<f64> {
        let mut vm = build_arrayed_reader(reader).expect("build arrayed reader");
        vm.run_to_end().expect("run");
        vm.get_series(&Ident::new("reader")).expect("reader")
    };
    let steady = |s: &[f64]| s[s.len() - 1];
    assert!((steady(&series("SUM(belt[a])")) - 400.0).abs() < 1e-6);
    assert!((steady(&series("SUM(belt[b])")) - 1000.0).abs() < 1e-6);
    assert!((steady(&series("MEAN(belt[a])")) - 25.0).abs() < 1e-6);
    assert!((steady(&series("MEAN(belt[b])")) - 62.5).abs() < 1e-6);
    assert!((steady(&series("SIZE(belt[a])")) - 16.0).abs() < 1e-9);
    // belt[b, 2]: slat 2 of belt b, 62.5 at steady state.
    assert!((steady(&series("belt[b, 2]")) - 62.5).abs() < 1e-6);
    // belt[a, 1]: exit slat of belt a, 25 at steady state.
    assert!((steady(&series("belt[a, 1]")) - 25.0).abs() < 1e-6);
}

#[test]
fn arrayed_bare_non_sum_reducer_still_rejected() {
    // A bare arrayed-conveyor reducer other than SUM has no single-belt
    // interpretation (it would read per-element TOTALS, not slats) -- it
    // stays loud-rejected (§10). SUM is the one spec-safe bare reduction.
    for eqn in [
        "MEAN(belt)",
        "MIN(belt)",
        "MAX(belt)",
        "STDDEV(belt)",
        "SIZE(belt)",
    ] {
        assert_eq!(
            build_arrayed_reader(eqn)
                .expect_err("bare arrayed non-SUM reducer should be rejected")
                .code,
            ErrorCode::ConveyorContainerAccessUnsupported,
            "equation '{eqn}'"
        );
    }
}

#[test]
fn ordinary_conveyor_simulation_unaffected_by_container_guard() {
    // The container-access guard must not perturb a conveyor model that uses
    // no container access: the steady-state oracle still holds exactly.
    let xml = include_str!("../../../test/conveyors/minimal_conveyor.xmile");
    let project = parse(xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build");
    vm.run_to_end().expect("run");
    let students = vm.get_series(&Ident::new("students")).expect("students");
    for &s in &students {
        assert!((s - 1000.0).abs() < 1e-6, "Students should hold at 1000");
    }
}

#[test]
fn equation_reading_driven_flow_is_rejected() {
    // An aux that reads the conveyor's outflow would see the placeholder 0
    // (the pass runs after flows), so expansion rejects it loudly.
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <aux name="reader"><eqn>out_f * 2</eqn></aux>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let err = build_vm(&project, &main).expect_err("reading a driven flow must be rejected");
    assert_eq!(err.code, ErrorCode::ConveyorDrivenFlowRead);
}

#[test]
fn conveyor_capacity_reading_driven_flow_is_rejected() {
    // A conveyor PARAMETER expression (here `<capacity>`) that references a
    // conveyor-driven flow (`attriting`, the belt's own leak outflow) escapes
    // the ordinary reader scan because the parameter is lifted into a hidden
    // aux appended only after that scan. It would silently compute from the
    // flow's Flows-phase placeholder 0 -- capacity 0, belt admits nothing, no
    // error. Expansion must reject it loudly, naming the conveyor PARAMETER
    // (not the internal `$conv$...` aux name) and the flow.
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>attriting</outflow>
      <conveyor><len>4</len><capacity>attriting * 20</capacity></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <flow name="attriting"><eqn>0.2</eqn><leak/></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let err =
        build_vm(&project, &main).expect_err("a capacity reading a driven flow must be rejected");
    assert_eq!(err.code, ErrorCode::ConveyorDrivenFlowRead);
    let msg = err.get_details().unwrap_or_default();
    assert!(
        msg.contains("belt") && msg.contains("<capacity>") && msg.contains("attriting"),
        "message should name the conveyor, the parameter, and the driven flow: {msg}"
    );
}

#[test]
fn conveyor_parameters_reading_driven_flow_are_rejected() {
    // Every conveyor parameter (`<len>`/`<capacity>`/`<in_limit>`/`<sample>`/
    // `<arrest>`) is lifted into a hidden aux; a reference to a driven flow in
    // ANY of them is the same placeholder-0 hazard (a zeroed cap/len, a never-
    // arrested belt, a sample/arrest condition read a step early). The scan
    // must cover all of them and name the offending parameter.
    for (label, block) in [
        ("<len>", "<len>attriting * 10</len>"),
        (
            "<capacity>",
            "<len>4</len><capacity>attriting * 20</capacity>",
        ),
        ("<in_limit>", "<len>4</len><in_limit>attriting</in_limit>"),
        ("<sample>", "<len>4</len><sample>attriting</sample>"),
        ("<arrest>", "<len>4</len><arrest>attriting</arrest>"),
    ] {
        let xml = wrap_model(&format!(
            r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>attriting</outflow>
      <conveyor>{block}</conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <flow name="attriting"><eqn>0.2</eqn><leak/></flow>"#,
        ));
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let err = build_vm(&project, &main).expect_err(&format!(
            "parameter {label} reading a driven flow must be rejected"
        ));
        assert_eq!(
            err.code,
            ErrorCode::ConveyorDrivenFlowRead,
            "parameter {label} should be rejected"
        );
        let msg = err.get_details().unwrap_or_default();
        assert!(
            msg.contains("belt") && msg.contains(label) && msg.contains("attriting"),
            "parameter {label}: message should name the conveyor, the parameter, and the flow: {msg}"
        );
    }
}

#[test]
fn conveyor_leak_fraction_reading_driven_flow_is_rejected() {
    // A leak's FRACTION is also lifted into a hidden aux. An explicit
    // `<leak>expr</leak>` fraction that references another driven flow (here
    // the primary outflow `out_f`) reads its placeholder 0 in the Flows phase
    // -- a silently-zeroed leak rate. Reject it, naming the leak flow.
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>1000</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>attriting</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <flow name="attriting"><eqn>0</eqn><leak>out_f * 0.001</leak></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let err = build_vm(&project, &main)
        .expect_err("a leak fraction reading a driven flow must be rejected");
    assert_eq!(err.code, ErrorCode::ConveyorDrivenFlowRead);
    let msg = err.get_details().unwrap_or_default();
    assert!(
        msg.contains("belt") && msg.contains("attriting") && msg.contains("out_f"),
        "message should name the conveyor, the leak flow, and the driven flow: {msg}"
    );
}

#[test]
fn conveyor_capacity_referencing_ordinary_aux_compiles_and_simulates() {
    // Negative control: a capacity that references an ORDINARY (non-driven)
    // aux is fine -- the aux is computed in the Flows phase like any other, so
    // the belt sees the real capacity. `cap_base * 2 = 600` plateaus contents
    // at 600, exactly like a literal `<capacity>600</capacity>` (S5).
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>4</len><capacity>cap_base * 2</capacity></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <aux name="cap_base"><eqn>300</eqn></aux>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build");
    vm.run_to_end().expect("run");
    let belt = vm.get_series(&Ident::new("belt")).expect("belt");
    for (i, &b) in belt.iter().enumerate() {
        assert!(b <= 600.0 + 1e-6, "step {i}: contents {b} exceeds capacity");
    }
    assert!(
        (belt[belt.len() - 1] - 600.0).abs() < 1e-6,
        "plateaus at 600: {}",
        belt[belt.len() - 1]
    );
}

// ----- multiple non-leak outflows (§3.3): the conveyor slat model has
// exactly one primary (belt-end) outflow plus leak flows. A second (or
// later) NON-leak outflow has no slat-model meaning; leaving it as an
// ordinary equation-driven outflow drains the expanded INTEG stock without
// removing material from the belt side table, so the reported stock and the
// belt total diverge with no diagnostic. Expansion rejects it loudly.

#[test]
fn second_non_leak_outflow_is_rejected() {
    // `graduating` is the primary (first non-leak, placeholder eqn 0);
    // `dropping_out` is a second non-leak outflow with a real rate. Before
    // the fix this simulated silently -- the Stocks phase drained `students`
    // by `dropping_out` while the belt never lost that material, diverging
    // permanently. It must be rejected, naming the conveyor and the extra.
    let xml = wrap_model(
        r#"
    <stock name="students"><eqn>0</eqn><inflow>matriculating</inflow><outflow>graduating</outflow><outflow>dropping_out</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="matriculating"><eqn>0</eqn></flow>
    <flow name="graduating"><eqn>0</eqn></flow>
    <flow name="dropping_out"><eqn>50</eqn></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let err = build_vm(&project, &main)
        .expect_err("a conveyor with two non-leak outflows must be rejected");
    assert_eq!(err.code, ErrorCode::ConveyorMultipleNonLeakOutflows);
    let msg = err.get_details().unwrap_or_default();
    assert!(
        msg.contains("students")
            && msg.contains("graduating")
            && msg.contains("dropping_out")
            && msg.contains("<leak/>"),
        "message should name the conveyor, the primary, the extra outflow, and the <leak/> fix: {msg}"
    );
}

#[test]
fn primary_plus_leak_outflow_still_compiles() {
    // Negative control: the SUPPORTED shape (one primary + one <leak/>
    // outflow) must still build and simulate -- the rejection targets only
    // extra NON-leak outflows. Mirrors `linear_leak_reaches_steady_state`.
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>attriting</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <flow name="attriting"><eqn>0.2</eqn><leak/></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("primary + leak must still build");
    vm.run_to_end().expect("run");
}

#[test]
fn primary_leak_and_extra_non_leak_rejects_naming_only_the_extra() {
    // Three outflows: `out_f` primary, `attriting` a <leak/>, `dropping` a
    // SECOND non-leak. The leak must NOT be misidentified as the extra: the
    // message names `dropping` (the real extra) and never `attriting`.
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>attriting</outflow><outflow>dropping</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <flow name="attriting"><eqn>0.2</eqn><leak/></flow>
    <flow name="dropping"><eqn>10</eqn></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let err =
        build_vm(&project, &main).expect_err("primary + leak + extra non-leak must be rejected");
    assert_eq!(err.code, ErrorCode::ConveyorMultipleNonLeakOutflows);
    let msg = err.get_details().unwrap_or_default();
    assert!(
        msg.contains("dropping"),
        "message should name the extra non-leak outflow `dropping`: {msg}"
    );
    assert!(
        !msg.contains("attriting"),
        "the <leak/> flow must NOT be listed as an extra outflow: {msg}"
    );
}

#[test]
fn two_conveyors_each_with_one_primary_compile() {
    // Negative control: two independent conveyors, each with a single
    // primary outflow, must not cross-talk into a false multiple-outflow
    // rejection. Both belts simulate.
    let xml = wrap_model(
        r#"
    <stock name="belt_a"><eqn>0</eqn><inflow>in_a</inflow><outflow>out_a</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_a"><eqn>250</eqn></flow>
    <flow name="out_a"><eqn>0</eqn></flow>
    <stock name="belt_b"><eqn>0</eqn><inflow>in_b</inflow><outflow>out_b</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_b"><eqn>100</eqn></flow>
    <flow name="out_b"><eqn>0</eqn></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("two single-primary conveyors must build");
    vm.run_to_end().expect("run");
    assert!(vm.get_series(&Ident::new("belt_a")).is_some());
    assert!(vm.get_series(&Ident::new("belt_b")).is_some());
}

// ----- slat-count bound (§4.1): a hostile/typo'd <len> must never
// panic/OOM the engine; it is rejected loudly at belt init / latch time.
// The tests shrink the bound with a `SlatBoundGuard` so a tiny fixture trips
// the gate without allocating a production-sized belt. At dt=0.25,
// `slat_count(transit) = round(transit/0.25)`: transit 1.0 -> 4 slats,
// transit 1.25 -> 5 slats.

/// A conveyor whose initial `<len>` needs more slats than the bound is
/// rejected at init (`init_belts`) with the new code, naming the belt.
#[test]
fn slat_bound_rejects_over_bound_transit_at_init() {
    let _guard = crate::conveyor::SlatBoundGuard::new(4);
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>1.25</len></conveyor></stock>
    <flow name="in_f"><eqn>10</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build");
    let err = vm
        .run_to_end()
        .expect_err("a transit needing 5 slats must be rejected against a bound of 4");
    assert_eq!(err.code, ErrorCode::ConveyorTransitTooLong);
    let msg = err.get_details().unwrap_or_default();
    assert!(
        msg.contains("belt") && msg.contains('5') && msg.contains('4'),
        "message should name the belt, the slat count, and the bound: {msg}"
    );
}

/// A conveyor whose initial `<len>` lands exactly ON the bound is admitted
/// (the gate rejects only counts strictly above the bound).
#[test]
fn slat_bound_admits_at_bound_transit_at_init() {
    let _guard = crate::conveyor::SlatBoundGuard::new(4);
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>1</len></conveyor></stock>
    <flow name="in_f"><eqn>10</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build");
    vm.run_to_end()
        .expect("a transit needing exactly 4 slats is at the bound, not over it");
}

/// The finding's exact abort case: a `<len>` of 1e300 whose `transit/dt`
/// saturates `usize`. The gate rejects it (loud error) instead of
/// `init_steady` panicking `vec![0.0; usize::MAX]` -- and, because the check
/// precedes the allocation, nothing near `usize::MAX` is ever allocated.
#[test]
fn slat_bound_rejects_saturating_transit_without_allocating() {
    let _guard = crate::conveyor::SlatBoundGuard::new(4);
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>1e300</len></conveyor></stock>
    <flow name="in_f"><eqn>10</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build");
    let err = vm
        .run_to_end()
        .expect_err("a usize-saturating transit must be rejected, not panic");
    assert_eq!(err.code, ErrorCode::ConveyorTransitTooLong);
}

/// A time-varying `<len>` (default `<sample>` re-latches every DT) that is
/// under the bound at init but grows over it mid-run must be rejected LOUDLY
/// from the runtime pass -- not silently clamp the belt geometry (repo rule:
/// a loud error beats a silently-wrong simulation). STEP raises `<len>` from
/// 1.0 (4 slats, at the bound) to 1.25 (5 slats, over it) at t=2.
#[test]
fn slat_bound_rejects_over_bound_latch_mid_run() {
    let _guard = crate::conveyor::SlatBoundGuard::new(4);
    let xml = wrap_model(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>1 + STEP(0.25, 2)</len></conveyor></stock>
    <flow name="in_f"><eqn>10</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build");
    let err = vm
        .run_to_end()
        .expect_err("a mid-run relatch needing 5 slats must be rejected against a bound of 4");
    assert_eq!(err.code, ErrorCode::ConveyorTransitTooLong);
}

// ----- discrete per-time-unit in_limit reset (v13 / §6.3) -----

#[test]
fn conveyor_time_unit_recovers_ideal_grid_boundary() {
    // Non-dyadic dt: ten 0.1 steps accumulate to 0.999...9 (floor 0), but the
    // per-time-unit boundary is 1 -- taken from the recovered ideal grid time,
    // not the drift-accumulated clock.
    let mut acc: f64 = 0.0;
    for _ in 0..10 {
        acc += 0.1;
    }
    assert_eq!(
        acc.floor() as i64,
        0,
        "precondition: additive drift floors to 0"
    );
    assert_eq!(conveyor_time_unit(acc, 0.0, 0.1), 1);

    // dt = 1/3: six steps sum to 1.9999999999999998 (floor 1), but the ideal
    // grid time 6*dt is exactly 2.0. (Three steps happen to round to exactly
    // 1.0, so the drift only bites at the k=6 boundary here.)
    let dt = 1.0 / 3.0;
    let mut acc: f64 = 0.0;
    for _ in 0..6 {
        acc += dt;
    }
    assert!(
        acc < 2.0,
        "precondition: additive sixths-of-a-third drift below 2.0"
    );
    assert_eq!(conveyor_time_unit(acc, 0.0, dt), 2);

    // Dyadic dt is exact: boundary detection is unchanged.
    assert_eq!(conveyor_time_unit(1.0, 0.0, 0.25), 1);
    assert_eq!(conveyor_time_unit(0.75, 0.0, 0.25), 0);

    // Step 0 returns floor(start): no spurious reset, matches the VM's
    // conveyor_last_unit seed.
    assert_eq!(conveyor_time_unit(0.0, 0.0, 0.1), 0);
    assert_eq!(conveyor_time_unit(2.5, 2.5, 0.1), 2);

    // A non-grid-aligned start whose grid points straddle (never hit) the
    // integers must NOT fire early: start=0.05, dt=0.1 -> t~=0.95 is unit 0,
    // t~=1.05 is unit 1.
    let start = 0.05;
    let mut acc = start;
    for _ in 0..9 {
        acc += 0.1;
    }
    assert_eq!(
        conveyor_time_unit(acc, start, 0.1),
        0,
        "t~=0.95 is still unit 0"
    );
    acc += 0.1;
    assert_eq!(conveyor_time_unit(acc, start, 0.1), 1, "t~=1.05 is unit 1");
}

#[test]
fn conveyor_time_unit_no_drift_over_long_run() {
    // The additive clock drifts both below and above the integers as it
    // accumulates; recovering the step index keeps every boundary on its ideal
    // grid step across a long run (500 steps).
    let dt = 0.1;
    let mut acc: f64 = 0.0;
    for step in 0..=500 {
        let want = (step as f64 * dt).floor() as i64;
        assert_eq!(
            conveyor_time_unit(acc, 0.0, dt),
            want,
            "step {step}: accumulated {acc} misdetects the time unit"
        );
        acc += dt;
    }
}

fn wrap_model_specs(vars: &str, start: f64, stop: f64, dt: f64) -> String {
    // `{start}`/`{stop}`/`{dt}` format via f64 Display, i.e. the shortest
    // round-trippable decimal, so the parsed dt is bit-identical to the f64
    // the assertions use (1.0/3.0 -> "0.3333333333333333" -> 1.0/3.0).
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
<options><uses_conveyor/></options></header>
  <sim_specs method="Euler" time_units="Months"><start>{start}</start><stop>{stop}</stop><dt>{dt}</dt></sim_specs>
  <model><variables>{vars}</variables></model>
</xmile>"#
    )
}

/// Build and run one discrete conveyor whose per-time-unit `in_limit` is 5,
/// fed by an abundant (100/unit) equation inflow that exhausts the budget in a
/// single step. The `in_f` slot holds the ADMITTED rate (admitted volume / dt)
/// after Phase B, so the returned series is a pulse of `5/dt` on the first step
/// of each integer time unit and 0 while that unit's budget is spent.
fn run_discrete_in_limit_series(start: f64, stop: f64, dt: f64) -> Vec<f64> {
    let vars = r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor discrete="true"><len>2</len><in_limit>5</in_limit></conveyor></stock>
    <flow name="in_f"><eqn>100</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#;
    let xml = wrap_model_specs(vars, start, stop, dt);
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build discrete in_limit vm");
    vm.run_to_end().expect("run discrete in_limit");
    vm.get_series(&Ident::new("in_f")).expect("in_f series")
}

/// Assert a discrete `in_limit=5` admission series (from
/// [`run_discrete_in_limit_series`], start = 0) is a pulse of `5/dt` on the
/// first step of each integer time unit and 0 elsewhere, admitting exactly 5
/// per unit. Step `k` models ideal time `k*dt`, so unit `u`'s first step is at
/// index `round(u/dt)`.
fn assert_discrete_pulses(series: &[f64], dt: f64, n_units: usize) {
    let pulse_rate = 5.0 / dt;
    for u in 0..n_units {
        let first = (u as f64 / dt).round() as usize;
        let next = ((u as f64 + 1.0) / dt).round() as usize;
        assert!(
            next <= series.len(),
            "unit {u} window [{first},{next}) exceeds series length {}",
            series.len()
        );
        let mut unit_vol = 0.0;
        for (offset, &rate) in series[first..next].iter().enumerate() {
            let k = first + offset;
            unit_vol += rate * dt;
            if k == first {
                assert!(
                    (rate - pulse_rate).abs() < 1e-6,
                    "unit {u} step {k} (t~={:.4}): admitted rate {rate}, want pulse {pulse_rate}",
                    k as f64 * dt
                );
            } else {
                assert!(
                    rate.abs() < 1e-9,
                    "unit {u} step {k} (t~={:.4}): admitted rate {rate}, want 0 (budget spent)",
                    k as f64 * dt
                );
            }
        }
        assert!(
            (unit_vol - 5.0).abs() < 1e-6,
            "unit {u}: admitted {unit_vol} over the unit, want exactly in_limit=5 (no double-fire)"
        );
    }
}

#[test]
fn discrete_in_limit_pulse_lands_on_integer_step_non_dyadic_dt() {
    // v13: with dt=0.1 the additive TIME clock sits just below each integer, so
    // the pre-fix floor(TIME) reset fired one dt late -- the step modeling
    // t=k.0 still saw the previous unit's exhausted budget and admitted 0, the
    // pulse instead landing at t~=k.1. The pulse must land on the t=k.0 step.
    let series = run_discrete_in_limit_series(0.0, 6.0, 0.1);
    assert_discrete_pulses(&series, 0.1, 6);
}

#[test]
fn discrete_in_limit_pulse_dyadic_dt_unchanged() {
    // A dyadic dt (0.25) is exactly representable: TIME never drifts and the
    // boundary detection is unchanged by the fix -- a no-regression guard.
    let series = run_discrete_in_limit_series(0.0, 4.0, 0.25);
    assert_discrete_pulses(&series, 0.25, 4);
}

#[test]
fn discrete_in_limit_reset_thirds_dt_lands_on_grid() {
    // dt=1/3: three steps sum to 0.999...9 (floor 0), but the ideal grid time
    // at k=3 is exactly 1.0, so the reset lands on step 3 (t=1), step 6 (t=2),
    // ... -- the first grid step at or past each integer.
    let dt = 1.0 / 3.0;
    let series = run_discrete_in_limit_series(0.0, 6.0, dt);
    assert_discrete_pulses(&series, dt, 6);
}

#[test]
fn discrete_in_limit_no_drift_over_long_run() {
    // Drift grows with the step count: run to t=50 (500 steps at dt=0.1) and
    // assert every time unit still admits exactly in_limit=5 on its first step
    // -- the reset never slips a dt late nor double-fires.
    let series = run_discrete_in_limit_series(0.0, 50.0, 0.1);
    assert_discrete_pulses(&series, 0.1, 50);
}

// ---- §7.2 explicit per-slat / per-time-unit list initialization ----

/// [`wrap_model_specs`] with start pinned to 0, for §7.2 tests whose slat
/// arithmetic reads cleanest at dt = 1 or dt = 0.5.
fn wrap_model_init(vars: &str, dt: f64, stop: f64) -> String {
    wrap_model_specs(vars, 0.0, stop, dt)
}

fn run_series(xml: &str, names: &[&str]) -> Vec<Vec<f64>> {
    let project = parse(xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build");
    vm.run_to_end().expect("run");
    names
        .iter()
        .map(|n| vm.get_series(&Ident::new(n)).expect(n))
        .collect()
}

#[test]
fn explicit_list_len_n_fills_slats_front_first() {
    // §7.2 length-N form: transit 3, dt 1 -> N = 3 slats; "10, 20, 30"
    // fills the belt directly, entry 1 at the front (exit) slat. With no
    // inflow the belt drains front-first: 60 -> 50 -> 30 -> 0, and the
    // driven outflow rate is 10, then 20, then 30.
    let xml = wrap_model_init(
        r#"
    <stock name="belt"><eqn>10, 20, 30</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>3</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
        1.0,
        5.0,
    );
    let series = run_series(&xml, &["belt", "out_f"]);
    let (belt, out) = (&series[0], &series[1]);
    for (i, want) in [60.0, 50.0, 30.0, 0.0, 0.0].iter().enumerate() {
        assert!(
            (belt[i] - want).abs() < 1e-9,
            "belt[{i}] = {} (want {want})",
            belt[i]
        );
    }
    // out_f[i] is the driven rate during step [i, i+1): the front (exit)
    // slat's 10 leaves first, then 20, then 30.
    for (i, want) in [10.0, 20.0, 30.0].iter().enumerate() {
        assert!(
            (out[i] - want).abs() < 1e-9,
            "out_f[{i}] = {} (want {want})",
            out[i]
        );
    }
}

#[test]
fn explicit_list_per_time_unit_spreads_continuous() {
    // §7.2 per-time-unit form: transit 2, dt 0.5 -> N = 4 slats but
    // U = floor(3 * 0.5) + 1 = 2 time-unit blocks, so a length-2 list is
    // one entry PER TIME UNIT: [40, 80] spreads to [20, 20, 40, 40]
    // (each block's entry split evenly across its slats on a continuous
    // conveyor, so the outflow during unit u totals v_u). Belt series:
    // 120 -> 100 -> 80 -> 40 -> 0.
    let xml = wrap_model_init(
        r#"
    <stock name="belt"><eqn>40, 80</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
        0.5,
        4.0,
    );
    let series = run_series(&xml, &["belt"]);
    let belt = &series[0];
    for (i, want) in [120.0, 100.0, 80.0, 40.0, 0.0].iter().enumerate() {
        assert!(
            (belt[i] - want).abs() < 1e-9,
            "belt[{i}] = {} (want {want})",
            belt[i]
        );
    }
}

#[test]
fn explicit_list_short_list_normalizes_and_stock_reports_belt_total() {
    // §7.2 normalization: transit 4, dt 1 -> N = U = 4; the length-2 list
    // [10, 20] repeats its last entry -> [10, 20, 20, 20], total 70. The
    // stock must report the NORMALIZED belt total (70) at t = 0, not the
    // raw list sum (30) -- and both INIT(belt) and a dependent initial
    // must see 70 (the post-belt-init reconcile).
    let xml = wrap_model_init(
        r#"
    <stock name="belt"><eqn>10, 20</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <aux name="init_check"><eqn>INIT(belt)</eqn></aux>
    <stock name="mirror"><eqn>belt</eqn></stock>"#,
        1.0,
        6.0,
    );
    let series = run_series(&xml, &["belt", "init_check", "mirror"]);
    let (belt, init_check, mirror) = (&series[0], &series[1], &series[2]);
    assert!(
        (belt[0] - 70.0).abs() < 1e-9,
        "belt[0] = {} (want the normalized belt total 70, not the raw sum 30)",
        belt[0]
    );
    for (i, &v) in init_check.iter().enumerate() {
        assert!(
            (v - 70.0).abs() < 1e-9,
            "init_check[{i}] = {v} (want INIT(belt) = 70)"
        );
    }
    assert!(
        (mirror[0] - 70.0).abs() < 1e-9,
        "mirror[0] = {} (a dependent initial must see the belt total 70)",
        mirror[0]
    );
}

#[test]
fn explicit_list_long_list_truncates() {
    // §7.2 normalization: transit 2, dt 1 -> N = U = 2; the length-3 list
    // [10, 20, 30] truncates to [10, 20], total 30 (raw sum 60).
    let xml = wrap_model_init(
        r#"
    <stock name="belt"><eqn>10, 20, 30</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
        1.0,
        4.0,
    );
    let series = run_series(&xml, &["belt"]);
    let belt = &series[0];
    assert!(
        (belt[0] - 30.0).abs() < 1e-9,
        "belt[0] = {} (want the truncated total 30, not the raw sum 60)",
        belt[0]
    );
    assert!((belt[1] - 20.0).abs() < 1e-9, "belt[1] = {}", belt[1]);
    assert!(belt[2].abs() < 1e-9, "belt[2] = {}", belt[2]);
}

#[test]
fn explicit_list_tolerates_whitespace_and_trailing_comma() {
    // Stella-authored lists may carry spaces and a trailing comma; both are
    // accepted (transit 3, dt 1 -> the length-3 direct fill of 60 total).
    let xml = wrap_model_init(
        r#"
    <stock name="belt"><eqn> 10 ,20,  30, </eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>3</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
        1.0,
        4.0,
    );
    let series = run_series(&xml, &["belt"]);
    assert!(
        (series[0][0] - 60.0).abs() < 1e-9,
        "belt[0] = {} (want 60)",
        series[0][0]
    );
}

#[test]
fn explicit_list_negative_entries_flow_through() {
    // The spec is silent on sign; scalar init (§7.1) accepts any V, so the
    // list form does too -- a negative entry rides the belt and exits as a
    // negative outflow rather than being silently clamped.
    let xml = wrap_model_init(
        r#"
    <stock name="belt"><eqn>-10, 20</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
        1.0,
        4.0,
    );
    let series = run_series(&xml, &["belt", "out_f"]);
    let (belt, out) = (&series[0], &series[1]);
    assert!((belt[0] - 10.0).abs() < 1e-9, "belt[0] = {}", belt[0]);
    assert!(
        (out[0] - -10.0).abs() < 1e-9,
        "out_f[0] = {} (the front slat's -10 exits first)",
        out[0]
    );
}

#[test]
fn explicit_list_non_constant_entry_rejected() {
    // A list-shaped <eqn> with a non-constant entry is rejected loudly at
    // expansion time (naming the entry), never silently steady-state
    // initialized and never surfaced as an opaque parse error.
    let xml = wrap_model_init(
        r#"
    <stock name="belt"><eqn>10, some_var, 30</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>3</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <aux name="some_var"><eqn>20</eqn></aux>"#,
        1.0,
        4.0,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let err = build_vm(&project, &main).expect_err("non-constant list entry must be rejected");
    assert_eq!(err.code, ErrorCode::ConveyorInitListUnsupported);
    let details = err.get_details().unwrap_or_default();
    assert!(
        details.contains("some_var"),
        "diagnostic should name the offending entry: {details}"
    );
}

#[test]
fn function_call_initial_is_not_mistaken_for_list() {
    // A scalar initial CONTAINING commas inside a call ("MAX(600, 300)") is
    // an ordinary §7.1 scalar init, not a §7.2 list: the belt steady-fills
    // to 600 and holds (inflow 150 = 600 / transit 4 keeps it steady).
    let xml = wrap_model_init(
        r#"
    <stock name="belt"><eqn>MAX(600, 300)</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>150</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
        1.0,
        8.0,
    );
    let series = run_series(&xml, &["belt"]);
    for (i, &b) in series[0].iter().enumerate() {
        assert!(
            (b - 600.0).abs() < 1e-6,
            "belt[{i}] = {b} (want steady 600)"
        );
    }
}

#[test]
fn arrayed_a2a_explicit_list_shared_across_belts() {
    // An apply-to-all arrayed conveyor shares one list across every element
    // belt (the same one-expression-per-attribute rule as <len>, §10):
    // both belts fill [10, 20] and drain independently.
    let xml = wrap_model_init(
        r#"
    <stock name="belt"><eqn>10, 20</eqn>
      <inflow>in_f</inflow><outflow>out_f</outflow>
      <dimensions><dim name="board"/></dimensions>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>
    <flow name="out_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>"#,
        1.0,
        4.0,
    )
    .replace(
        "<model>",
        "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
    );
    let series = run_series(&xml, &["belt[a]", "belt[b]"]);
    for (name, belt) in [("belt[a]", &series[0]), ("belt[b]", &series[1])] {
        for (i, want) in [30.0, 20.0, 0.0].iter().enumerate() {
            assert!(
                (belt[i] - want).abs() < 1e-9,
                "{name}[{i}] = {} (want {want})",
                belt[i]
            );
        }
    }
}

#[test]
fn arrayed_per_element_explicit_lists_fill_each_belt() {
    // A non-apply-to-all arrayed conveyor gives each element its own <eqn>
    // (XMILE 4.5.2: element equations "are allowed to vary between
    // elements"), and a conveyor stock's <eqn> is its initial -- so a
    // per-element list initializes THAT element's belt directly (§7.2).
    // transit 2, dt 1 -> 2 slats: belt[a] = [10, 20] drains 30 -> 20 -> 0
    // (outflow 10 then 20); belt[b] = [5, 40] drains 45 -> 40 -> 0.
    let xml = wrap_model_init(
        r#"
    <stock name="belt">
      <inflow>in_f</inflow><outflow>out_f</outflow>
      <dimensions><dim name="board"/></dimensions>
      <element subscript="a"><eqn>10, 20</eqn></element>
      <element subscript="b"><eqn>5, 40</eqn></element>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>
    <flow name="out_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>"#,
        1.0,
        4.0,
    )
    .replace(
        "<model>",
        "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
    );
    let series = run_series(&xml, &["belt[a]", "belt[b]", "out_f[a]", "out_f[b]"]);
    for (name, belt, wants) in [
        ("belt[a]", &series[0], [30.0, 20.0, 0.0]),
        ("belt[b]", &series[1], [45.0, 40.0, 0.0]),
    ] {
        for (i, want) in wants.iter().enumerate() {
            assert!(
                (belt[i] - want).abs() < 1e-9,
                "{name}[{i}] = {} (want {want})",
                belt[i]
            );
        }
    }
    for (name, out, wants) in [
        ("out_f[a]", &series[2], [10.0, 20.0, 0.0]),
        ("out_f[b]", &series[3], [5.0, 40.0, 0.0]),
    ] {
        for (i, want) in wants.iter().enumerate() {
            assert!(
                (out[i] - want).abs() < 1e-9,
                "{name}[{i}] = {} (want {want})",
                out[i]
            );
        }
    }
}

#[test]
fn arrayed_mixed_list_and_scalar_elements() {
    // Mixing forms is well-defined because each element belt is independent:
    // element a's list [10, 20] fills its belt directly (§7.2) while element
    // b's ordinary scalar 40 steady-fills [20, 20] over the 2 slats (§7.1).
    let xml = wrap_model_init(
        r#"
    <stock name="belt">
      <inflow>in_f</inflow><outflow>out_f</outflow>
      <dimensions><dim name="board"/></dimensions>
      <element subscript="a"><eqn>10, 20</eqn></element>
      <element subscript="b"><eqn>40</eqn></element>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>
    <flow name="out_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>"#,
        1.0,
        4.0,
    )
    .replace(
        "<model>",
        "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
    );
    let series = run_series(&xml, &["belt[a]", "belt[b]"]);
    for (name, belt, wants) in [
        ("belt[a]", &series[0], [30.0, 20.0, 0.0]),
        ("belt[b]", &series[1], [40.0, 20.0, 0.0]),
    ] {
        for (i, want) in wants.iter().enumerate() {
            assert!(
                (belt[i] - want).abs() < 1e-9,
                "{name}[{i}] = {} (want {want})",
                belt[i]
            );
        }
    }
}

#[test]
fn arrayed_per_element_lists_match_by_subscript_not_position() {
    // The dimension declares its elements as (b, a), so belt[b] is row-major
    // element 0 -- while the <element> blocks are written a-first. The lists
    // must land by canonical SUBSCRIPT match, never by Vec position.
    let xml = wrap_model_init(
        r#"
    <stock name="belt">
      <inflow>in_f</inflow><outflow>out_f</outflow>
      <dimensions><dim name="board"/></dimensions>
      <element subscript="a"><eqn>10, 20</eqn></element>
      <element subscript="b"><eqn>1, 2</eqn></element>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>
    <flow name="out_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>"#,
        1.0,
        4.0,
    )
    .replace(
        "<model>",
        "<dimensions><dim name=\"board\"><elem name=\"b\"/><elem name=\"a\"/></dim></dimensions><model>",
    );
    let series = run_series(&xml, &["belt[a]", "belt[b]"]);
    for (name, belt, wants) in [
        ("belt[a]", &series[0], [30.0, 20.0, 0.0]),
        ("belt[b]", &series[1], [3.0, 2.0, 0.0]),
    ] {
        for (i, want) in wants.iter().enumerate() {
            assert!(
                (belt[i] - want).abs() < 1e-9,
                "{name}[{i}] = {} (want {want})",
                belt[i]
            );
        }
    }
}

#[test]
fn arrayed_per_element_lists_normalize_independently() {
    // §7.2 per-time-unit normalization runs per belt: transit 4, dt 1 ->
    // N = U = 4, so a's short list [10, 20] extends to [10, 20, 20, 20]
    // (total 70) and b's [1, 2] to [1, 2, 2, 2] (total 7). Each element's
    // placeholder must carry ITS OWN normalized total, so an init-time
    // consumer (a no-flow mirror stock) sees 70 / 7, not raw sums 30 / 3.
    let xml = wrap_model_init(
        r#"
    <stock name="belt">
      <inflow>in_f</inflow><outflow>out_f</outflow>
      <dimensions><dim name="board"/></dimensions>
      <element subscript="a"><eqn>10, 20</eqn></element>
      <element subscript="b"><eqn>1, 2</eqn></element>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>
    <flow name="out_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>
    <stock name="mirror_a"><eqn>belt[a]</eqn></stock>
    <stock name="mirror_b"><eqn>belt[b]</eqn></stock>"#,
        1.0,
        6.0,
    )
    .replace(
        "<model>",
        "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
    );
    let series = run_series(&xml, &["belt[a]", "belt[b]", "mirror_a", "mirror_b"]);
    assert!(
        (series[0][0] - 70.0).abs() < 1e-9,
        "belt[a][0] = {} (want the normalized 70, not the raw sum 30)",
        series[0][0]
    );
    assert!(
        (series[1][0] - 7.0).abs() < 1e-9,
        "belt[b][0] = {} (want the normalized 7, not the raw sum 3)",
        series[1][0]
    );
    assert!(
        (series[2][0] - 70.0).abs() < 1e-9,
        "mirror_a[0] = {} (an init-time consumer must see element a's total 70)",
        series[2][0]
    );
    assert!(
        (series[3][0] - 7.0).abs() < 1e-9,
        "mirror_b[0] = {} (an init-time consumer must see element b's total 7)",
        series[3][0]
    );
}

#[test]
fn arrayed_list_chained_consumers_see_per_element_normalized_totals() {
    // The arrayed twin of the scalar placeholder-leak regressions above
    // (`chained_conveyor_initialized_from_list_stock_sees_normalized_total`
    // etc.): transit 4, dt 1 normalizes a's [10, 20] to [10, 20, 20, 20]
    // (70) and b's [1, 2] to [1, 2, 2, 2] (7), and every init-time consumer
    // must see the PER-ELEMENT normalized totals, never the raw sums
    // 30 / 3:
    // - a downstream CONVEYOR seeded from belt[a] steady-fills 70 over its
    //   2 slats ([35, 35]) and returns the full 70 through its outflow;
    // - a downstream QUEUE seeded from belt[b] holds a 7 batch that agrees
    //   with SUM(waiting);
    // - SUM(belt[*]) captured at init is 70 + 7 = 77.
    let vars = r#"
    <stock name="belt">
      <inflow>in_f</inflow><outflow>out_f</outflow>
      <dimensions><dim name="board"/></dimensions>
      <element subscript="a"><eqn>10, 20</eqn></element>
      <element subscript="b"><eqn>1, 2</eqn></element>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>
    <flow name="out_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>
    <stock name="chain"><eqn>belt[a]</eqn><inflow>in_c</inflow><outflow>out_c</outflow>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_c"><eqn>0</eqn></flow>
    <flow name="out_c"><eqn>0</eqn></flow>
    <stock name="waiting"><eqn>belt[b]</eqn><inflow>arrivals</inflow><outflow>into_service</outflow>
      <queue/></stock>
    <flow name="arrivals"><eqn>0</eqn><non_negative/></flow>
    <flow name="into_service"><eqn>0</eqn></flow>
    <aux name="q_total"><eqn>SUM(waiting)</eqn></aux>
    <stock name="total0"><eqn>SUM(belt[*])</eqn></stock>"#;
    let xml = wrap_model_init(vars, 1.0, 6.0).replace(
        "<model>",
        "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build");
    vm.run_to_end().expect("run");
    let chain = vm.get_series(&Ident::new("chain")).expect("chain");
    let out_c = vm.get_series(&Ident::new("out_c")).expect("out_c");
    let waiting = vm.get_series(&Ident::new("waiting")).expect("waiting");
    let q_total = vm.get_series(&Ident::new("q_total")).expect("q_total");
    let total0 = vm.get_series(&Ident::new("total0")).expect("total0");
    for (i, want) in [70.0, 35.0, 0.0, 0.0].iter().enumerate() {
        assert!(
            (chain[i] - want).abs() < 1e-9,
            "chain[{i}] = {} (want {want}; a chained conveyor must fill from \
             element a's normalized 70, not the raw 30)",
            chain[i]
        );
    }
    let drained: f64 = out_c[0] + out_c[1];
    assert!(
        (drained - 70.0).abs() < 1e-9,
        "chain outflow must return the full normalized 70, got {drained}"
    );
    assert!(
        (waiting[0] - 7.0).abs() < 1e-9,
        "waiting[0] = {} (want element b's normalized 7, not the raw 3)",
        waiting[0]
    );
    assert!(
        (q_total[0] - 7.0).abs() < 1e-9,
        "SUM(waiting)[0] = {} (want 7; the FIFO batch must match the stock)",
        q_total[0]
    );
    assert!(
        (total0[0] - 77.0).abs() < 1e-9,
        "SUM(belt[*]) at init = {} (want 70 + 7 = 77)",
        total0[0]
    );
}

#[test]
fn arrayed_2d_per_element_lists_match_by_subscript_across_dims() {
    // 2-D canonical-subscript matching: BOTH dimensions declare their
    // elements in an order different from the <element> block order (row =
    // (r2, r1), col = (cb, ca), while the blocks are written r1-first and
    // ca-first), so a positional match would misassign every belt. transit
    // 2, dt 1: each belt must hold exactly its own list's total and drain
    // its own second entry.
    let xml = wrap_model_init(
        r#"
    <stock name="belt">
      <inflow>in_f</inflow><outflow>out_f</outflow>
      <dimensions><dim name="row"/><dim name="col"/></dimensions>
      <element subscript="r1,ca"><eqn>10, 20</eqn></element>
      <element subscript="r1,cb"><eqn>1, 2</eqn></element>
      <element subscript="r2,ca"><eqn>100, 200</eqn></element>
      <element subscript="r2,cb"><eqn>4, 5</eqn></element>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn><dimensions><dim name="row"/><dim name="col"/></dimensions></flow>
    <flow name="out_f"><eqn>0</eqn><dimensions><dim name="row"/><dim name="col"/></dimensions></flow>"#,
        1.0,
        4.0,
    )
    .replace(
        "<model>",
        "<dimensions>\
         <dim name=\"row\"><elem name=\"r2\"/><elem name=\"r1\"/></dim>\
         <dim name=\"col\"><elem name=\"cb\"/><elem name=\"ca\"/></dim>\
         </dimensions><model>",
    );
    let series = run_series(
        &xml,
        &["belt[r1,ca]", "belt[r1,cb]", "belt[r2,ca]", "belt[r2,cb]"],
    );
    for (name, belt, wants) in [
        ("belt[r1,ca]", &series[0], [30.0, 20.0, 0.0]),
        ("belt[r1,cb]", &series[1], [3.0, 2.0, 0.0]),
        ("belt[r2,ca]", &series[2], [300.0, 200.0, 0.0]),
        ("belt[r2,cb]", &series[3], [9.0, 5.0, 0.0]),
    ] {
        for (i, want) in wants.iter().enumerate() {
            assert!(
                (belt[i] - want).abs() < 1e-9,
                "{name}[{i}] = {} (want {want})",
                belt[i]
            );
        }
    }
}

#[test]
fn arrayed_default_list_applies_to_unlisted_elements() {
    // A top-level <eqn> coexisting with <element> blocks is the EXCEPT
    // default equation; when it is itself a list it initializes every
    // element WITHOUT an explicit entry (b here), while a's own list wins
    // for a. transit 2, dt 1: belt[a] = [10, 20], belt[b] = [1, 2].
    let xml = wrap_model_init(
        r#"
    <stock name="belt">
      <eqn>1, 2</eqn>
      <inflow>in_f</inflow><outflow>out_f</outflow>
      <dimensions><dim name="board"/></dimensions>
      <element subscript="a"><eqn>10, 20</eqn></element>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>
    <flow name="out_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>"#,
        1.0,
        4.0,
    )
    .replace(
        "<model>",
        "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
    );
    let series = run_series(&xml, &["belt[a]", "belt[b]"]);
    for (name, belt, wants) in [
        ("belt[a]", &series[0], [30.0, 20.0, 0.0]),
        ("belt[b]", &series[1], [3.0, 2.0, 0.0]),
    ] {
        for (i, want) in wants.iter().enumerate() {
            assert!(
                (belt[i] - want).abs() < 1e-9,
                "{name}[{i}] = {} (want {want})",
                belt[i]
            );
        }
    }
}

#[test]
fn arrayed_per_element_list_bad_entry_rejected() {
    // A per-element list with a non-constant entry is still rejected loudly,
    // and the diagnostic names both the offending entry and the element.
    let xml = wrap_model_init(
        r#"
    <stock name="belt">
      <inflow>in_f</inflow><outflow>out_f</outflow>
      <dimensions><dim name="board"/></dimensions>
      <element subscript="a"><eqn>10, some_var</eqn></element>
      <element subscript="b"><eqn>5, 5</eqn></element>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>
    <flow name="out_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>
    <aux name="some_var"><eqn>20</eqn></aux>"#,
        1.0,
        4.0,
    )
    .replace(
        "<model>",
        "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let err = build_vm(&project, &main).expect_err("non-constant per-element entry rejected");
    assert_eq!(err.code, ErrorCode::ConveyorInitListUnsupported);
    let details = err.get_details().unwrap_or_default();
    assert!(
        details.contains("some_var") && details.contains("belt[a]"),
        "diagnostic should name the entry and the element: {details}"
    );
}

#[test]
fn arrayed_per_element_list_with_non_constant_transit_rejected() {
    // The literal-<len> requirement (§7.2) applies to per-element lists
    // exactly as it does to shared ones: the per-belt normalization needs
    // the slat count at expansion time.
    let xml = wrap_model_init(
        r#"
    <stock name="belt">
      <inflow>in_f</inflow><outflow>out_f</outflow>
      <dimensions><dim name="board"/></dimensions>
      <element subscript="a"><eqn>10, 20</eqn></element>
      <element subscript="b"><eqn>5, 5</eqn></element>
      <conveyor><len>tt</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>
    <flow name="out_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>
    <aux name="tt"><eqn>2</eqn></aux>"#,
        1.0,
        4.0,
    )
    .replace(
        "<model>",
        "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let err = build_vm(&project, &main).expect_err("non-literal transit must be rejected");
    assert_eq!(err.code, ErrorCode::ConveyorInitListUnsupported);
}

#[test]
fn arrayed_per_element_list_produces_no_error_diagnostics() {
    // The editor diagnostic path parses the UN-expanded project; a valid
    // per-element list initial must not surface as a spurious parse error
    // (the runtime path accepts it, so diagnostics must too).
    let xml = wrap_model_init(
        r#"
    <stock name="belt">
      <inflow>in_f</inflow><outflow>out_f</outflow>
      <dimensions><dim name="board"/></dimensions>
      <element subscript="a"><eqn>10, 20</eqn></element>
      <element subscript="b"><eqn>5, 40</eqn></element>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>
    <flow name="out_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>"#,
        1.0,
        4.0,
    )
    .replace(
        "<model>",
        "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
    );
    let project = parse(&xml);
    let mut db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel_incremental(&mut db, &project, None);
    let diags = crate::db::collect_all_diagnostics(&db, sync.project);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == crate::db::DiagnosticSeverity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "a valid per-element list initial must not produce Error diagnostics: {errors:?}"
    );
}

#[test]
fn stella_attribute_form_header_simulates_from_per_stock_block() {
    // Stella writes NO <options> block: both vendored .stmx fixtures declare
    // conveyor usage only as attributes on the <smile> header element
    // (`uses_conveyor=""`). The reader deliberately ignores that advisory
    // form (conveyors.md 3.1) -- the per-stock <conveyor> block is
    // authoritative -- so such a file must open and simulate as a conveyor:
    // initial 10 == inflow 5 x transit 2 holds the belt steady at 10 with a
    // steady outflow of 5.
    let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0" xmlns:isee="http://iseesystems.com/XMILE" uses_conveyor="">
  <header>
    <smile version="1.0" namespace="std, isee" uses_arrays="1" uses_conveyor="" uses_submodels=""/>
    <name>t</name><vendor>isee systems, inc.</vendor>
    <product version="2.0" lang="en">Stella Architect</product>
  </header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>6</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="belt"><eqn>10</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_f"><eqn>5</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
  </variables></model>
</xmile>"#;
    let series = run_series(xml, &["belt", "out_f"]);
    let (belt, out) = (&series[0], &series[1]);
    for (i, &b) in belt.iter().enumerate() {
        assert!((b - 10.0).abs() < 1e-9, "belt[{i}] = {b} (want steady 10)");
    }
    for (i, &o) in out.iter().enumerate().skip(1) {
        assert!((o - 5.0).abs() < 1e-9, "out_f[{i}] = {o} (want steady 5)");
    }
}

#[test]
fn explicit_list_survives_xmile_roundtrip() {
    // The reader must preserve the list <eqn> verbatim on the datamodel
    // (the expansion rewrites only its private clone) and the writer must
    // re-emit it, so a §7.2 model round-trips losslessly.
    let xml = wrap_model_init(
        r#"
    <stock name="belt"><eqn>10, 20, 30</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>3</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
        1.0,
        4.0,
    );
    let project = parse(&xml);
    let stock_eqn = project.models[0]
        .variables
        .iter()
        .find_map(|v| match v {
            datamodel::Variable::Stock(s) if s.ident == "belt" => Some(s.equation.clone()),
            _ => None,
        })
        .expect("belt stock");
    assert_eq!(stock_eqn, Equation::Scalar("10, 20, 30".to_string()));

    // Building a VM must not mutate the caller's project.
    let main = project.models[0].name.clone();
    build_vm(&project, &main).expect("build");
    let after = project.models[0]
        .variables
        .iter()
        .find_map(|v| match v {
            datamodel::Variable::Stock(s) if s.ident == "belt" => Some(s.equation.clone()),
            _ => None,
        })
        .expect("belt stock");
    assert_eq!(after, Equation::Scalar("10, 20, 30".to_string()));

    let out = crate::compat::to_xmile(&project).expect("to_xmile");
    assert!(
        out.contains("10, 20, 30"),
        "writer must re-emit the list eqn: {out}"
    );
}

#[test]
fn explicit_list_produces_no_error_diagnostics() {
    // The editor diagnostic path parses the UN-expanded project; a valid
    // §7.2 list initial must not surface as a spurious equation parse
    // error there (the runtime path accepts it, so diagnostics must too).
    let xml = wrap_model_init(
        r#"
    <stock name="belt"><eqn>10, 20, 30</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>3</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
        1.0,
        4.0,
    );
    let project = parse(&xml);
    let mut db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel_incremental(&mut db, &project, None);
    let diags = crate::db::collect_all_diagnostics(&db, sync.project);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == crate::db::DiagnosticSeverity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "a valid explicit-list initial must not produce Error diagnostics: {errors:?}"
    );
}

/// The list conveyor every placeholder-leak regression below builds on:
/// transit 4, dt 1 -> N = U = 4; the short list [10, 20] normalizes to
/// [10, 20, 20, 20] = 70 while its RAW sum is 30. Any init-time consumer
/// that sees 30 instead of 70 reproduces the leak.
const LIST_BELT_A: &str = r#"
    <stock name="belt_a"><eqn>10, 20</eqn><inflow>in_a</inflow><outflow>out_a</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_a"><eqn>0</eqn></flow>
    <flow name="out_a"><eqn>0</eqn></flow>"#;

#[test]
fn chained_conveyor_initialized_from_list_stock_sees_normalized_total() {
    // A SECOND conveyor whose scalar initial reads the list stock must
    // steady-fill from the normalized total (70), not the raw list sum
    // (30): belt init runs BEFORE the post-init reconcile, so a raw-sum
    // placeholder would fill belt_b's belt from 30 while the reconcile
    // bumps its STOCK slot to 70 -- a belt permanently inconsistent with
    // its own stock (stock stuck at 40 with an empty belt, conservation
    // violated). A correct fill is 70 spread over 2 slats: [35, 35], so
    // the series drains 70 -> 35 -> 0 and the outflow returns the full 70.
    let vars = format!(
        r#"{LIST_BELT_A}
    <stock name="belt_b"><eqn>belt_a</eqn><inflow>in_b</inflow><outflow>out_b</outflow>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_b"><eqn>0</eqn></flow>
    <flow name="out_b"><eqn>0</eqn></flow>"#
    );
    let xml = wrap_model_init(&vars, 1.0, 6.0);
    let series = run_series(&xml, &["belt_b", "out_b"]);
    let (belt_b, out_b) = (&series[0], &series[1]);
    for (i, want) in [70.0, 35.0, 0.0, 0.0].iter().enumerate() {
        assert!(
            (belt_b[i] - want).abs() < 1e-9,
            "belt_b[{i}] = {} (want {want}); belt/stock inconsistent",
            belt_b[i]
        );
    }
    let drained: f64 = out_b[0] + out_b[1];
    assert!(
        (drained - 70.0).abs() < 1e-9,
        "outflow must return the full normalized 70, got {drained}"
    );
}

#[test]
fn queue_seeded_from_list_stock_sees_normalized_total() {
    // A queue whose initial reads the list stock seeds its FIFO in
    // init_queues, which runs after belt init but reads the stock slot the
    // INITIALS pass wrote -- so a raw-sum placeholder seeds a 30 batch
    // while the reconcile reports the stock as 70: SUM(waiting) and
    // waiting disagree at t = 0. Both must be the normalized 70.
    let vars = format!(
        r#"{LIST_BELT_A}
    <stock name="waiting"><eqn>belt_a</eqn><inflow>arrivals</inflow><outflow>into_service</outflow>
      <queue/></stock>
    <flow name="arrivals"><eqn>0</eqn><non_negative/></flow>
    <flow name="into_service"><eqn>0</eqn></flow>
    <aux name="q_total"><eqn>SUM(waiting)</eqn></aux>"#
    );
    let xml = wrap_model_init(&vars, 1.0, 4.0);
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let mut vm = crate::queue_compile::build_vm(&project, &main).expect("build");
    vm.run_to_end().expect("run");
    let waiting = vm.get_series(&Ident::new("waiting")).expect("waiting");
    let q_total = vm.get_series(&Ident::new("q_total")).expect("q_total");
    assert!(
        (waiting[0] - 70.0).abs() < 1e-9,
        "waiting[0] = {} (want 70)",
        waiting[0]
    );
    assert!(
        (q_total[0] - 70.0).abs() < 1e-9,
        "SUM(waiting)[0] = {} (want 70; the FIFO batch must match the stock)",
        q_total[0]
    );
}

#[test]
fn transit_reading_list_stock_uses_normalized_total() {
    // A conveyor whose <len> references the list stock evaluates it in the
    // pre-init_belts Flows run -- before any write-back -- so a raw-sum
    // placeholder yields transit 30/35 ~= 0.86 (1 slat) instead of
    // 70/35 = 2 (2 slats). With 2 slats a steady 70 drains [35, 35].
    let vars = format!(
        r#"{LIST_BELT_A}
    <stock name="belt_c"><eqn>70</eqn><inflow>in_c</inflow><outflow>out_c</outflow>
      <conveyor><len>belt_a / 35</len></conveyor></stock>
    <flow name="in_c"><eqn>0</eqn></flow>
    <flow name="out_c"><eqn>0</eqn></flow>"#
    );
    let xml = wrap_model_init(&vars, 1.0, 6.0);
    let series = run_series(&xml, &["belt_c"]);
    let belt_c = &series[0];
    for (i, want) in [70.0, 35.0, 0.0].iter().enumerate() {
        assert!(
            (belt_c[i] - want).abs() < 1e-9,
            "belt_c[{i}] = {} (want {want}; transit must see 70/35 = 2)",
            belt_c[i]
        );
    }
}

#[test]
fn explicit_list_with_non_constant_transit_rejected() {
    // The list-length interpretation (per-slat vs per-time-unit) and the
    // stock's compile-time initial total both depend on the slat count, so
    // a list-initialized conveyor requires a compile-time-constant
    // <len>. A runtime transit expression is rejected loudly.
    let xml = wrap_model_init(
        r#"
    <stock name="belt"><eqn>10, 20</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>tt</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <aux name="tt"><eqn>4</eqn></aux>"#,
        1.0,
        4.0,
    );
    let project = parse(&xml);
    let main = project.models[0].name.clone();
    let err = build_vm(&project, &main).expect_err("non-literal transit must be rejected");
    assert_eq!(err.code, ErrorCode::ConveyorInitListUnsupported);
    let details = err.get_details().unwrap_or_default();
    assert!(
        details.contains("numeric literal"),
        "diagnostic should explain the literal-transit requirement: {details}"
    );
}

#[test]
fn model_sim_specs_dt_override_drives_list_normalization() {
    // The runtime prefers the ROOT MODEL's sim_specs override over the
    // project's (assemble.rs "preferring model-level sim_specs"), so the
    // expansion-time §7.2 probe must read the same dt. Project dt = 1 but
    // the model overrides dt = 1.5: transit 2 at dt 1.5 -> N =
    // round(2/1.5) = 1 slat, U = 1, so the 2-entry list truncates to [10]
    // and the stock must report 10. A probe reading PROJECT dt (1) sizes
    // N = 2 (direct fill) and bakes 30 into the placeholder -- caught by
    // the init_belts debug_assert in debug, silent divergence in release.
    let xml = wrap_model_init(
        r#"
    <stock name="belt"><eqn>10, 20</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
        1.0,
        6.0,
    );
    let mut project = parse(&xml);
    project.models[0].sim_specs = Some(datamodel::SimSpecs {
        start: 0.0,
        stop: 6.0,
        dt: datamodel::Dt::Dt(1.5),
        save_step: None,
        sim_method: datamodel::SimMethod::Euler,
        time_units: None,
    });
    let main = project.models[0].name.clone();
    let mut vm = build_vm(&project, &main).expect("build");
    vm.run_to_end().expect("run");
    let belt = vm.get_series(&Ident::new("belt")).expect("belt");
    assert!(
        (belt[0] - 10.0).abs() < 1e-9,
        "belt[0] = {} (want 10, the dt-1.5 one-slat truncation; a project-dt probe bakes 30)",
        belt[0]
    );
}

#[test]
fn model_sim_specs_rk4_override_rejected_for_conveyor() {
    // The Euler-only gate must read the ROOT MODEL's sim_specs override,
    // not just the project's: a model-level RK4 override would otherwise
    // evade ConveyorNonEulerMethod and integrate the belt under RK.
    let xml = wrap_model_init(
        r#"
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>2</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
        1.0,
        4.0,
    );
    let mut project = parse(&xml);
    project.models[0].sim_specs = Some(datamodel::SimSpecs {
        start: 0.0,
        stop: 4.0,
        dt: datamodel::Dt::Dt(0.25),
        save_step: None,
        sim_method: datamodel::SimMethod::RungeKutta4,
        time_units: None,
    });
    let main = project.models[0].name.clone();
    let err = build_vm(&project, &main).expect_err("model-level RK4 override must be rejected");
    assert_eq!(err.code, ErrorCode::ConveyorNonEulerMethod);
}

#[test]
fn explicit_list_single_entry_with_trailing_comma() {
    // "5," is a one-entry list (the trailing comma is what makes it a
    // list): per §7.2 short-list normalization the single entry repeats
    // for every time unit -> [5, 5, 5] on a 3-slat dt-1 belt, total 15.
    // Before this was handled it fell through to an opaque parse error
    // while "10, 20," was accepted.
    let xml = wrap_model_init(
        r#"
    <stock name="belt"><eqn>5,</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>3</len></conveyor></stock>
    <flow name="in_f"><eqn>0</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>"#,
        1.0,
        5.0,
    );
    let series = run_series(&xml, &["belt"]);
    let belt = &series[0];
    for (i, want) in [15.0, 10.0, 5.0, 0.0].iter().enumerate() {
        assert!(
            (belt[i] - want).abs() < 1e-9,
            "belt[{i}] = {} (want {want})",
            belt[i]
        );
    }
}
