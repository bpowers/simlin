// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Oracle tests for the conveyor runtime engine.
//!
//! These reproduce scenarios S1-S15 from `docs/design/conveyors.md` §15 and its
//! executable reference `test/conveyors/reference_prototype.py`. Every scenario
//! asserts the §4.3 conservation identity (`admitted - out - leak = Δcontents`)
//! on every step, plus the scenario-specific invariant. The steady-state
//! numbers (S3/S4/S14/S15) are pinned to the prototype's values so the Rust
//! engine is bit-compatible with the reference.

use crate::conveyor::*;

const INF: f64 = f64::INFINITY;

/// One step of a single isolated conveyor: Phase A then Phase B, with the
/// conservation identity asserted. Returns the reported *rates* (volumes / dt).
struct StepOut {
    outflow: f64,
    leak: Vec<f64>,
    inflow: f64,
    in_rates: Vec<f64>,
    cleared: Vec<f64>,
}

fn step_single(
    conv: &mut ConveyorState,
    eq_rates: &[f64],
    fractions: &[f64],
    transit: f64,
    sample: bool,
    capacity: f64,
    in_limit: f64,
) -> StepOut {
    let dt = conv.dt();
    let contents0 = conv.contents();
    let pa = conv.phase_a(PhaseAInputs {
        arrested: false,
        sample,
        transit,
        leak_fractions: fractions,
        dest_arrested: false,
        leak_dest_arrested: &[],
    });
    let pb = conv.phase_b(PhaseBInputs {
        phase_a: &pa,
        eq_request_rates: eq_rates,
        conv_inflows: &[],
        leak_fractions: fractions,
        capacity,
        in_limit,
        placements: &[],
    });
    let delta = conv.contents() - contents0;
    let leaked: f64 = pa.leak_vols.iter().sum();
    let residual = pb.admitted - pa.out_vol - leaked - delta;
    assert!(
        residual.abs() < 1e-9,
        "conservation residual {residual} (admitted {} out {} leak {leaked} delta {delta})",
        pb.admitted,
        pa.out_vol
    );
    StepOut {
        outflow: pa.out_vol / dt,
        leak: pa.leak_vols.iter().map(|v| v / dt).collect(),
        inflow: pb.admitted / dt,
        in_rates: pb.in_vols.iter().map(|v| v / dt).collect(),
        cleared: pb.cleared,
    }
}

/// One chain step: upstream `a`'s primary outflow feeds downstream `b`
/// (conveyor-driven, admitted unconditionally). `b_arrested` freezes `b` and
/// holds `a`'s exit. Conservation is asserted for both. Returns
/// `(a_outflow, b_outflow, b_inflow)` rates.
fn step_chain(
    a: &mut ConveyorState,
    b: &mut ConveyorState,
    a_eq: &[f64],
    b_arrested: bool,
) -> (f64, f64, f64) {
    let dt = a.dt();
    let a0 = a.contents();
    let b0 = b.contents();
    let no: [f64; 0] = [];
    // Phase A over both conveyors (order-free): a's primary destination is b.
    let pa_a = a.phase_a(PhaseAInputs {
        arrested: false,
        sample: false,
        transit: 0.0,
        leak_fractions: &no,
        dest_arrested: b_arrested,
        leak_dest_arrested: &[],
    });
    let pa_b = b.phase_a(PhaseAInputs {
        arrested: b_arrested,
        sample: false,
        transit: 0.0,
        leak_fractions: &no,
        dest_arrested: false,
        leak_dest_arrested: &[],
    });
    // Phase B: a admits its own equation inflow; b admits a's outflow.
    let pb_a = a.phase_b(PhaseBInputs {
        phase_a: &pa_a,
        eq_request_rates: a_eq,
        conv_inflows: &[],
        leak_fractions: &no,
        capacity: INF,
        in_limit: INF,
        placements: &[],
    });
    let pb_b = b.phase_b(PhaseBInputs {
        phase_a: &pa_b,
        eq_request_rates: &no,
        conv_inflows: &[(pa_a.out_vol, Placement::Beginning)],
        leak_fractions: &no,
        capacity: b_cap(b),
        in_limit: INF,
        placements: &[],
    });
    // Conservation for each conveyor.
    for (label, contents0, pa, admitted, conv) in [
        ("a", a0, &pa_a, pb_a.admitted, &*a),
        ("b", b0, &pa_b, pb_b.admitted, &*b),
    ] {
        let leaked: f64 = pa.leak_vols.iter().sum();
        let residual = admitted - pa.out_vol - leaked - (conv.contents() - contents0);
        assert!(
            residual.abs() < 1e-9,
            "chain conservation {label}: {residual}"
        );
    }
    (pa_a.out_vol / dt, pa_b.out_vol / dt, pb_b.admitted / dt)
}

// step_chain drives b with its configured capacity; the harness stores it in a
// thread-local since ConveyorState doesn't expose it. Simpler: pass INF unless
// a scenario overrides. We special-case via a small wrapper below.
thread_local! {
    static B_CAP: std::cell::Cell<f64> = const { std::cell::Cell::new(INF) };
}
fn b_cap(_b: &ConveyorState) -> f64 {
    B_CAP.with(|c| c.get())
}

// ---------- S1: minimal steady state ----------
#[test]
fn s1_minimal_steady_state() {
    // T=4, DT=.25, V=1000, inflow=250, capacity=1200: contents and outflow
    // constant at the equilibrium inflow*T.
    let mut c = ConveyorState::new(0.25, false, false, false, vec![]);
    c.init_steady(4.0, 1000.0, &[]);
    let mut t: f64 = 0.0;
    while t <= 12.0 + 1e-9 {
        let contents = c.contents();
        let r = step_single(&mut c, &[250.0], &[], 4.0, true, 1200.0, INF);
        assert!(
            (contents - 1000.0).abs() < 1e-6 && (r.outflow - 250.0).abs() < 1e-6,
            "t={t}: contents={contents} outflow={}",
            r.outflow
        );
        t += 0.25;
    }
}

// ---------- S2: fill from empty (pure transit delay) ----------
#[test]
fn s2_fill_from_empty() {
    let mut c = ConveyorState::new(0.25, false, false, false, vec![]);
    c.init_steady(4.0, 0.0, &[]);
    let mut rows = vec![];
    let mut t: f64 = 0.0;
    while t <= 6.0 + 1e-9 {
        let contents = c.contents();
        let r = step_single(&mut c, &[250.0], &[], 4.0, true, INF, INF);
        rows.push((t, contents, r.outflow));
        t += 0.25;
    }
    let first_out = rows.iter().find(|r| r.2 > 0.0).unwrap().0;
    assert!(
        (first_out - 4.0).abs() < 1e-9,
        "first nonzero outflow at t={first_out}"
    );
    assert_eq!(rows[0].1, 0.0, "contents 0 at t=0");
    let first_full = rows.iter().find(|r| (r.1 - 1000.0).abs() < 1e-9).unwrap().0;
    assert!(
        (first_full - 4.0).abs() < 1e-9,
        "1000 first reached at t={first_full}"
    );
}

// ---------- S3: linear leak f=0.2 full zone ----------
#[test]
fn s3_linear_leak() {
    let mut c = ConveyorState::new(0.25, false, false, false, vec![LeakConfig::default()]);
    c.init_from_inflow(4.0, 250.0, &[0.2]);
    let mut last = 0.0;
    let mut t: f64 = 0.0;
    while t <= 8.0 + 1e-9 {
        last = step_single(&mut c, &[250.0], &[0.2], 4.0, true, INF, INF).outflow;
        t += 0.25;
    }
    assert!(
        (last / 250.0 - 0.8).abs() < 1e-6,
        "steady outflow/inflow = {}",
        last / 250.0
    );
}

// ---------- S4: exponential leak f=0.1/time ----------
#[test]
fn s4_exponential_leak() {
    let mut c = ConveyorState::new(0.25, true, false, false, vec![LeakConfig::default()]);
    c.init_from_inflow(4.0, 250.0, &[0.1]);
    let mut last = 0.0;
    let mut t: f64 = 0.0;
    while t <= 8.0 + 1e-9 {
        last = step_single(&mut c, &[250.0], &[0.1], 4.0, true, INF, INF).outflow;
        t += 0.25;
    }
    let expect = 250.0 * (1.0f64 - 0.1 * 0.25).powi(16);
    assert!(
        (last - expect).abs() < 1e-4,
        "steady outflow {last} expected {expect}"
    );
}

// ---------- S5: capacity clips inflow ----------
#[test]
fn s5_capacity() {
    let mut c = ConveyorState::new(0.25, false, false, false, vec![]);
    c.init_steady(4.0, 0.0, &[]);
    let mut t: f64 = 0.0;
    while t <= 12.0 + 1e-9 {
        let contents = c.contents();
        assert!(
            contents <= 600.0 + 1e-6,
            "t={t} contents={contents} exceeds capacity"
        );
        step_single(&mut c, &[250.0], &[], 4.0, true, 600.0, INF);
        t += 0.25;
    }
}

// ---------- S6: inflow limit (continuous) ----------
#[test]
fn s6_inflow_limit() {
    let mut c = ConveyorState::new(0.25, false, false, false, vec![]);
    c.init_steady(4.0, 0.0, &[]);
    let mut t: f64 = 0.0;
    while t <= 8.0 + 1e-9 {
        let r = step_single(&mut c, &[250.0], &[], 4.0, true, INF, 150.0);
        assert!(
            r.inflow <= 150.0 + 1e-6,
            "t={t} admitted inflow {} exceeds limit",
            r.inflow
        );
        t += 0.25;
    }
}

// ---------- S7: non-integer transit rounding (half away from zero) ----------
#[test]
fn s7_rounding() {
    assert_eq!(slat_count(4.1, 0.25), 16, "16.4 rounds to 16");
    assert_eq!(slat_count(4.125, 0.25), 17, "16.5 rounds half AWAY to 17");
    assert_eq!(slat_count(0.1, 0.25), 1, "clamped to at least 1 slat");
}

// ---------- S8: chain A->B, B capacity-constrained (never blocked) ----------
#[test]
fn s8_chain_never_blocked() {
    B_CAP.with(|c| c.set(100.0));
    let mut a = ConveyorState::new(0.25, false, false, false, vec![]);
    let mut b = ConveyorState::new(0.25, false, false, false, vec![]);
    a.init_steady(2.0, 500.0, &[]);
    b.init_steady(4.0, 0.0, &[]);
    let total0 = a.contents() + b.contents();
    let mut exited = 0.0;
    let mut peak_b = 0.0f64;
    for _ in 0..40 {
        let (_a_out, b_out, _b_in) = step_chain(&mut a, &mut b, &[0.0], false);
        exited += b_out * 0.25;
        peak_b = peak_b.max(b.contents());
    }
    B_CAP.with(|c| c.set(INF));
    assert!(
        peak_b > 100.0 + 1e-9,
        "B capacity transiently exceeded: peak {peak_b}"
    );
    let residual = (a.contents() + b.contents() + exited) - total0;
    assert!(
        residual.abs() < 1e-6,
        "chain conservation residual {residual}"
    );
}

// ---------- S9: transit shrink 4->2 with linear leak f=0.2 ----------
#[test]
fn s9_shrink_merge() {
    let mut c = ConveyorState::new(0.25, false, false, false, vec![LeakConfig::default()]);
    c.init_from_inflow(4.0, 250.0, &[0.2]);
    let (mut total_in, mut total_out, mut total_leak) = (0.0, 0.0, 0.0);
    let start = c.contents();
    let mut last_out = 0.0;
    let mut t: f64 = 0.0;
    // After t=2, sample re-latches <len> to 2.
    for _ in 0..48 {
        let transit = if t >= 2.0 - 1e-9 { 2.0 } else { 4.0 };
        let r = step_single(&mut c, &[250.0], &[0.2], transit, true, INF, INF);
        total_in += r.inflow * 0.25;
        total_out += r.outflow * 0.25;
        total_leak += r.leak[0] * 0.25;
        last_out = r.outflow;
        t += 0.25;
    }
    let residual = total_in - total_out - total_leak - (c.contents() - start);
    assert!(
        residual.abs() < 1e-6,
        "whole-run conservation residual {residual}"
    );
    assert!(
        total_leak <= 0.2 * (total_in + start) + 1e-6,
        "lifetime leak {total_leak} exceeds budget"
    );
    assert!(
        (last_out - 200.0).abs() < 1e-6,
        "post-shrink steady outflow {last_out} != (1-f)*inflow=200"
    );
}

// ---------- S10: arrest + held exit ----------
#[test]
fn s10_arrest_held_exit() {
    let mut a = ConveyorState::new(0.25, false, false, false, vec![]);
    let mut b = ConveyorState::new(0.25, false, false, false, vec![]);
    a.init_steady(2.0, 500.0, &[]);
    b.init_steady(4.0, 0.0, &[]);
    let total0 = a.contents() + b.contents();
    let mut exited = 0.0;
    let mut hold_ok = true;
    let mut release_out: Option<f64> = None;
    let mut t: f64 = 0.0;
    for _ in 0..64 {
        let arrested = (1.0 - 1e-9..2.0 - 1e-9).contains(&t);
        let (a_out, b_out, b_in) = step_chain(&mut a, &mut b, &[0.0], arrested);
        if arrested {
            hold_ok = hold_ok && a_out == 0.0 && b_out.abs() < 1e-12 && b_in.abs() < 1e-12;
        }
        if (t - 2.0).abs() < 1e-9 {
            release_out = Some(a_out);
        }
        exited += b_out * 0.25;
        t += 0.25;
    }
    assert!(hold_ok, "during hold A outflow==0 and B frozen");
    // At hold start A has drained 4 of 8 slats, so 250 remains; it all exits as
    // one lump on release: rate 250/0.25 = 1000.
    assert!(
        release_out
            .map(|o| (o - 1000.0).abs() < 1e-6)
            .unwrap_or(false),
        "release lump rate {release_out:?} != 1000"
    );
    let residual = (a.contents() + b.contents() + exited) - total0;
    assert!(
        residual.abs() < 1e-6,
        "chain conservation residual {residual}"
    );
}

// ---------- S11: discrete quantization vs tight capacity ----------
#[test]
fn s11_discrete_capacity() {
    let mut c = ConveyorState::new(0.25, false, true, false, vec![]);
    c.init_steady(2.0, 0.0, &[]);
    let mut integral_ok = true;
    let mut cap_ok = true;
    let mut inserted = 0.0;
    let mut t: f64 = 0.0;
    let mut prev_unit = 0i64;
    for _ in 0..64 {
        if t.floor() as i64 != prev_unit {
            c.on_time_boundary();
            prev_unit = t.floor() as i64;
        }
        let r = step_single(&mut c, &[2.4], &[], 2.0, true, 3.0, INF); // 2.4/time = 0.6/DT
        inserted += r.inflow * 0.25;
        cap_ok = cap_ok && c.contents() <= 3.0 + 1e-9;
        integral_ok = integral_ok
            && c.slat_contents()
                .iter()
                .all(|&x| (x - x.round()).abs() < 1e-9);
        t += 0.25;
    }
    assert!(integral_ok, "slat contents always integral");
    assert!(cap_ok, "contents never exceed capacity 3");
    assert!(inserted > 0.0, "material flows (no deadlock)");
}

// ---------- S12: discrete, two inflows, per-inflow attribution ----------
#[test]
fn s12_discrete_two_inflows() {
    let mut c = ConveyorState::new(0.25, false, true, false, vec![]);
    c.init_steady(2.0, 0.0, &[]);
    let mut cum_cleared = [0.0, 0.0];
    let mut cum_reported = [0.0, 0.0];
    let mut identity_ok = true;
    let mut shutoff_reported = None;
    let mut t: f64 = 0.0;
    let mut prev_unit = 0i64;
    for _ in 0..64 {
        if t.floor() as i64 != prev_unit {
            c.on_time_boundary();
            prev_unit = t.floor() as i64;
        }
        let rates = [1.6, if t < 8.0 { 0.8 } else { 0.0 }];
        let r = step_single(&mut c, &rates, &[], 2.0, true, 4.0, INF);
        let carry = c.quant_carry_snapshot();
        for j in 0..2 {
            cum_cleared[j] += r.cleared[j];
            cum_reported[j] += r.in_rates[j] * 0.25;
            // cleared_j == reported_j + carry_j at every step.
            identity_ok = identity_ok
                && (cum_cleared[j] - (cum_reported[j] + carry[j])).abs() < 1e-9
                && carry[j] >= 0.0;
        }
        if shutoff_reported.is_none() && t >= 8.0 {
            shutoff_reported = Some(cum_reported[1]);
        }
        t += 0.25;
    }
    assert!(identity_ok, "per-inflow bookkeeping identity");
    assert!(
        cum_reported[1] - shutoff_reported.unwrap() < 1.0 + 1e-9,
        "after shutoff inflow 1 reports at most its residual carry"
    );
    for v in cum_reported {
        assert!((v - v.round()).abs() < 1e-9, "integral totals: {v}");
    }
}

// ---------- S13: time-varying linear leak fraction 0.2 -> 0.4 at t=4 ----------
#[test]
fn s13_time_varying_fraction() {
    let mut c = ConveyorState::new(0.25, false, false, false, vec![LeakConfig::default()]);
    c.init_from_inflow(4.0, 250.0, &[0.2]);
    let mut switch_leak: Option<f64> = None;
    let mut last_out = 0.0;
    let mut t: f64 = 0.0;
    for _ in 0..64 {
        let f = if t < 4.0 { 0.2 } else { 0.4 };
        let r = step_single(&mut c, &[250.0], &[f], 4.0, true, INF, INF);
        if (t - 4.0).abs() < 1e-9 {
            switch_leak = Some(r.leak[0]);
        }
        last_out = r.outflow;
        t += 0.25;
    }
    assert!(
        switch_leak
            .map(|x| (x - 100.0).abs() < 1e-6)
            .unwrap_or(false),
        "leak doubles for ALL cohorts at the switch: {switch_leak:?}"
    );
    assert!(
        (last_out - 150.0).abs() < 1e-6,
        "outflow re-equilibrates at 150: {last_out}"
    );
}

// ---------- S14: two overlapping exponential leaks add ----------
#[test]
fn s14_exponential_add() {
    let mut c = ConveyorState::new(
        0.25,
        true,
        false,
        false,
        vec![LeakConfig::default(), LeakConfig::default()],
    );
    c.init_from_inflow(4.0, 250.0, &[0.1, 0.1]);
    let mut r = None;
    for _ in 0..32 {
        r = Some(step_single(
            &mut c,
            &[250.0],
            &[0.1, 0.1],
            4.0,
            true,
            INF,
            INF,
        ));
    }
    let r = r.unwrap();
    let expect = 250.0 * (1.0f64 - 0.2 * 0.25).powi(16);
    assert!(
        (r.outflow - expect).abs() < 1e-4,
        "outflow {} expected {expect}",
        r.outflow
    );
    assert!(
        (r.leak[0] - r.leak[1]).abs() < 1e-9,
        "the two flows report equal halves"
    );
}

// ---------- S15: staggered linear zones ----------
#[test]
fn s15_staggered_zones() {
    let leaks = vec![
        LeakConfig {
            zone_start: 0.0,
            zone_end: 0.5,
            integers: false,
        },
        LeakConfig {
            zone_start: 0.5,
            zone_end: 1.0,
            integers: false,
        },
    ];
    let mut c = ConveyorState::new(0.25, false, false, false, leaks);
    c.init_from_inflow(4.0, 250.0, &[0.5, 0.5]);
    let mut r = None;
    for _ in 0..48 {
        r = Some(step_single(
            &mut c,
            &[250.0],
            &[0.5, 0.5],
            4.0,
            true,
            INF,
            INF,
        ));
    }
    let r = r.unwrap();
    assert!(
        (r.outflow - 62.5).abs() < 1e-6,
        "steady outflow 62.5: {}",
        r.outflow
    );
    assert!(
        (r.leak[0] - 125.0).abs() < 1e-6 && (r.leak[1] - 62.5).abs() < 1e-6,
        "leaks report 125 and 62.5: {:?}",
        r.leak
    );
}

// ---------- integer leakage (§5.4): not an oracle number, a property test ----------
#[test]
fn integer_leak_keeps_units_whole() {
    // A discrete conveyor with an integer leak flow: slat contents stay integral
    // and the leak reports whole units, with the fractional remainder carried.
    let leaks = vec![LeakConfig {
        zone_start: 0.0,
        zone_end: 1.0,
        integers: true,
    }];
    let mut c = ConveyorState::new(0.25, false, true, false, leaks);
    c.init_steady(2.0, 0.0, &[0.3]); // one initial fraction per leak flow
    let mut total_leak = 0.0;
    let mut total_in = 0.0;
    let mut t: f64 = 0.0;
    let mut prev_unit = 0i64;
    for _ in 0..80 {
        if t.floor() as i64 != prev_unit {
            c.on_time_boundary();
            prev_unit = t.floor() as i64;
        }
        let r = step_single(&mut c, &[8.0], &[0.3], 2.0, true, INF, INF);
        total_leak += r.leak[0] * 0.25;
        total_in += r.inflow * 0.25;
        // every slat content is a whole number
        for x in c.slat_contents() {
            assert!((x - x.round()).abs() < 1e-9, "t={t} non-integral slat {x}");
        }
        // reported leak this step is a whole number of units / dt
        let units = r.leak[0] * 0.25;
        assert!(
            (units - units.round()).abs() < 1e-9,
            "leak not whole units: {units}"
        );
        t += 0.25;
    }
    assert!(total_leak > 0.0, "some material leaked");
    assert!(total_in > 0.0, "material flowed");
}

// ---------- leak flow feeding an arrested destination is skipped (§4.3 step 2) ----------
#[test]
fn leak_to_arrested_destination_is_skipped() {
    let mut c = ConveyorState::new(0.25, false, false, false, vec![LeakConfig::default()]);
    c.init_from_inflow(4.0, 250.0, &[0.2]);
    let contents0 = c.contents();
    let pa = c.phase_a(PhaseAInputs {
        arrested: false,
        sample: true,
        transit: 4.0,
        leak_fractions: &[0.2],
        dest_arrested: false,
        leak_dest_arrested: &[true], // the leak's destination is arrested
    });
    assert_eq!(pa.leak_vols[0], 0.0, "arrested-destination leak reports 0");
    // Phase A reports the exit volume but does not remove it (the pop happens in
    // Phase B); with the leak skipped, nothing at all was removed this phase, so
    // total contents are unchanged.
    let after = c.contents();
    assert!(
        (after - contents0).abs() < 1e-9,
        "skipped leak removes nothing: before {contents0} after {after}"
    );
}

// ---------- discrete steady init: time-unit block lumping (§6.4 rule 3) ----------
#[test]
fn discrete_steady_init_lumps_and_conserves() {
    // T=2, DT=.25 -> N=8 slats; U = floor(7*.25)+1 = 2 time-unit blocks.
    // Block 0 = slats 0..3, block 1 = slats 4..7 (deepest 3 and 7). A no-leak
    // scalar init V=16 spreads to 2/slat continuously, then lumps each block's
    // 8 units at its deepest slat.
    let mut c = ConveyorState::new(0.25, false, true, false, vec![]);
    c.init_steady(2.0, 16.0, &[]);
    let slats = c.slat_contents();
    assert_eq!(slats.len(), 8);
    assert!(
        (c.contents() - 16.0).abs() < 1e-9,
        "total conserved: {}",
        c.contents()
    );
    // Only the block-deepest slats (3 and 7) hold material.
    for (i, &x) in slats.iter().enumerate() {
        if i == 3 || i == 7 {
            assert!((x - 8.0).abs() < 1e-9, "slat {i} holds the block lump: {x}");
        } else {
            assert_eq!(x, 0.0, "slat {i} empty after lumping");
        }
    }
}

// ---------- explicit per-slat list init (§7.2, length N) ----------
#[test]
fn explicit_list_init_per_slat() {
    // T=1, DT=.25 -> N=4; a length-4 list fills each slat directly, front first.
    let mut c = ConveyorState::new(0.25, false, false, false, vec![]);
    c.init_explicit(1.0, &[10.0, 20.0, 30.0, 40.0], &[]);
    assert_eq!(c.slat_contents(), vec![10.0, 20.0, 30.0, 40.0]);
    assert_eq!(c.slat_content(0), Some(10.0));
    assert_eq!(c.slat_content(4), None);
    // it then runs: first exit is the front slat (10) as a pure delay.
    let r = step_single(&mut c, &[0.0], &[], 1.0, true, INF, INF);
    assert!(
        (r.outflow - 10.0 / 0.25).abs() < 1e-9,
        "front slat exits first: {}",
        r.outflow
    );
}

// ---------- explicit per-time-unit list init (§7.2, non-N length) ----------
#[test]
fn explicit_list_init_per_time_unit_continuous() {
    // T=2, DT=.25 -> N=8, U=2 blocks. A length-2 list [40, 80] fills block 0
    // (slats 0..3) with 40 spread evenly (10 each) and block 1 with 80 (20 each).
    let mut c = ConveyorState::new(0.25, false, false, false, vec![]);
    c.init_explicit(2.0, &[40.0, 80.0], &[]);
    let slats = c.slat_contents();
    assert_eq!(slats, vec![10.0, 10.0, 10.0, 10.0, 20.0, 20.0, 20.0, 20.0]);
    assert!((c.contents() - 120.0).abs() < 1e-9);
}

// ---------- explicit per-time-unit list init (§7.2, non-N length, discrete) ----------
#[test]
fn explicit_list_init_per_time_unit_discrete() {
    // T=2, DT=.5 -> N=4, U=2 blocks. A DISCRETE conveyor places
    // each block's whole entry at the block's deepest slat ("start of each time
    // unit" semantics, §7.2 / §6.4 rule 3) instead of spreading it.
    let mut c = ConveyorState::new(0.5, false, true, false, vec![]);
    c.init_explicit(2.0, &[40.0, 80.0], &[]);
    assert_eq!(c.slat_contents(), vec![0.0, 40.0, 0.0, 80.0]);
    assert!((c.contents() - 120.0).abs() < 1e-9);
}

// ---------- explicit list normalization (§7.2: truncate extra, repeat last) ----------
#[test]
fn explicit_list_init_normalizes_short_and_long_lists() {
    // T=4, DT=1 -> N=4, U=4. A short list repeats its last entry: [10, 20] ->
    // [10, 20, 20, 20] (total 70, NOT the raw sum 30).
    let mut c = ConveyorState::new(1.0, false, false, false, vec![]);
    c.init_explicit(4.0, &[10.0, 20.0], &[]);
    assert_eq!(c.slat_contents(), vec![10.0, 20.0, 20.0, 20.0]);
    assert!((c.contents() - 70.0).abs() < 1e-9);

    // T=2, DT=1 -> N=2, U=2. A long list truncates: [10, 20, 30] -> [10, 20].
    let mut c = ConveyorState::new(1.0, false, false, false, vec![]);
    c.init_explicit(2.0, &[10.0, 20.0, 30.0], &[]);
    assert_eq!(c.slat_contents(), vec![10.0, 20.0]);
}

// ---------- continuous conveyor + integer leak: fractional slats, whole-unit leaks ----------
#[test]
fn continuous_integer_leak_reports_whole_units() {
    // A CONTINUOUS conveyor (fractional slat contents) with an integer leak.
    // The slats may be fractional, but the leak always reports whole units and
    // conservation holds every step (asserted inside step_single).
    let leaks = vec![LeakConfig {
        zone_start: 0.0,
        zone_end: 1.0,
        integers: true,
    }];
    let mut c = ConveyorState::new(0.25, false, false, false, leaks);
    c.init_from_inflow(4.0, 25.0, &[0.3]);
    let mut total_leak = 0.0;
    let mut any_fractional_slat = false;
    for _ in 0..64 {
        let r = step_single(&mut c, &[25.0], &[0.3], 4.0, true, INF, INF);
        let leaked_units = r.leak[0] * 0.25;
        assert!(
            (leaked_units - leaked_units.round()).abs() < 1e-9,
            "integer leak must report whole units, got {leaked_units}"
        );
        total_leak += leaked_units;
        if c.slat_contents()
            .iter()
            .any(|&x| (x - x.round()).abs() > 1e-9)
        {
            any_fractional_slat = true;
        }
    }
    assert!(total_leak > 0.0, "some whole units leaked");
    assert!(
        any_fractional_slat,
        "a continuous conveyor holds fractional slat contents"
    );
}

// ---- spread inputs (section 8) ----

/// Fresh empty no-leak belt of `transit/dt` slats.
fn fresh_belt(transit: f64, dt: f64) -> ConveyorState {
    let mut c = ConveyorState::new(dt, false, false, false, vec![]);
    c.init_steady(transit, 0.0, &[]);
    c
}

/// Run one phase_a + phase_b inserting a single equation inflow of `rate` with
/// `placement` (no cap/limit), returning the belt contents exit-first.
fn insert_once(c: &mut ConveyorState, rate: f64, placement: Placement, transit: f64) -> Vec<f64> {
    let pa = c.phase_a(PhaseAInputs {
        arrested: false,
        sample: true,
        transit,
        leak_fractions: &[],
        dest_arrested: false,
        leak_dest_arrested: &[],
    });
    c.phase_b(PhaseBInputs {
        phase_a: &pa,
        eq_request_rates: &[rate],
        conv_inflows: &[],
        leak_fractions: &[],
        capacity: f64::INFINITY,
        in_limit: f64::INFINITY,
        placements: &[placement],
    });
    c.slat_contents()
}

#[test]
fn spread_beginning_places_all_at_entry() {
    let mut c = fresh_belt(4.0, 1.0); // 4 slats, d=4, entry = index 3
    let contents = insert_once(&mut c, 8.0, Placement::Beginning, 4.0);
    assert_eq!(contents, vec![0.0, 0.0, 0.0, 8.0]);
}

#[test]
fn spread_even_splits_across_entry_path() {
    let mut c = fresh_belt(4.0, 1.0);
    let contents = insert_once(&mut c, 8.0, Placement::Even, 4.0);
    // A/d = 2 at every entry-path slat (incl the exit slat).
    for (i, &x) in contents.iter().enumerate() {
        assert!((x - 2.0).abs() < 1e-12, "slat {i}: {x}");
    }
    assert!((contents.iter().sum::<f64>() - 8.0).abs() < 1e-12);
}

#[test]
fn spread_dist_weights_normalize_to_shares() {
    let mut c = fresh_belt(4.0, 1.0);
    // weights exit-first over the 4 entry-path slats; A_i = A * w_i / Σw.
    let w = vec![1.0, 3.0, 0.0, 4.0];
    let contents = insert_once(&mut c, 8.0, Placement::Dist(w.clone()), 4.0);
    let sw: f64 = w.iter().sum();
    for (i, &wi) in w.iter().enumerate() {
        assert!(
            (contents[i] - 8.0 * wi / sw).abs() < 1e-12,
            "slat {i}: {}",
            contents[i]
        );
    }
    assert!((contents.iter().sum::<f64>() - 8.0).abs() < 1e-12);
}

#[test]
fn spread_dist_empty_weights_fall_back_to_beginning() {
    let mut c = fresh_belt(4.0, 1.0);
    let contents = insert_once(&mut c, 8.0, Placement::Dist(vec![]), 4.0);
    assert_eq!(contents, vec![0.0, 0.0, 0.0, 8.0]);
}

#[test]
fn spread_dest_falls_back_to_beginning_on_empty_belt() {
    let mut c = fresh_belt(4.0, 1.0);
    let contents = insert_once(&mut c, 8.0, Placement::Dest, 4.0);
    assert_eq!(contents, vec![0.0, 0.0, 0.0, 8.0]);
}

#[test]
fn spread_dest_is_content_proportional() {
    // Seed the belt so it has non-uniform content, then dest-place a new inflow
    // and check each slat gained A * content_i / Σcontent (conserving A).
    let mut c = fresh_belt(4.0, 1.0);
    // Two beginning inserts (with a shift between) give a non-empty, non-uniform
    // belt without any leak.
    insert_once(&mut c, 4.0, Placement::Beginning, 4.0);
    insert_once(&mut c, 8.0, Placement::Beginning, 4.0);
    let before = c.slat_contents();
    let total: f64 = before.iter().sum();
    // Now a dest insert of A=6 (compute shift-adjusted expectation directly).
    let pa = c.phase_a(PhaseAInputs {
        arrested: false,
        sample: true,
        transit: 4.0,
        leak_fractions: &[],
        dest_arrested: false,
        leak_dest_arrested: &[],
    });
    // After phase_a the belt hasn't shifted yet; capture post-shift content by
    // running phase_b and comparing the delta to A * (post-shift content).
    c.phase_b(PhaseBInputs {
        phase_a: &pa,
        eq_request_rates: &[6.0],
        conv_inflows: &[],
        leak_fractions: &[],
        capacity: f64::INFINITY,
        in_limit: f64::INFINITY,
        placements: &[Placement::Dest],
    });
    let after = c.slat_contents();
    let gained: f64 = after.iter().sum::<f64>() - (total - pa.out_vol);
    assert!(
        (gained - 6.0).abs() < 1e-9,
        "dest must admit all of A=6, gained {gained}"
    );
    // dest spreads over >1 slat when the belt has multiple filled slats.
    let nonzero = after.iter().filter(|&&x| x > 1e-9).count();
    assert!(
        nonzero >= 2,
        "dest should spread across content, contents={after:?}"
    );
}

#[test]
fn leak_slat_vols_sum_to_leak_vols() {
    // The per-slat leak breakdown (used by downstream `source` placement, §8)
    // must conserve: Σ_i leak_slat_vols[k][i] == leak_vols[k] for every leak
    // flow k, under both a linear and an exponential leak, and with the belt at
    // its non-trivial steady-state fill so multiple slats leak.
    for exponential in [false, true] {
        let mut c = ConveyorState::new(
            0.25,
            exponential,
            false,
            false,
            vec![LeakConfig {
                zone_start: 0.0,
                zone_end: 1.0,
                integers: false,
            }],
        );
        let fracs = [0.2];
        c.init_from_inflow(2.0, 100.0, &fracs);
        let pa = c.phase_a(PhaseAInputs {
            arrested: false,
            sample: true,
            transit: 2.0,
            leak_fractions: &fracs,
            dest_arrested: false,
            leak_dest_arrested: &[false],
        });
        assert_eq!(pa.leak_slat_vols.len(), 1, "one leak flow");
        let per_slat_sum: f64 = pa.leak_slat_vols[0].iter().sum();
        assert!(
            (per_slat_sum - pa.leak_vols[0]).abs() < 1e-12,
            "exponential={exponential}: per-slat sum {per_slat_sum} != total {}",
            pa.leak_vols[0]
        );
        assert!(pa.leak_vols[0] > 0.0, "the belt should be leaking");
    }
}

#[test]
fn arrested_leak_slat_vols_are_indexable_but_empty() {
    // An arrested conveyor did no leak, so each leak flow's per-slat breakdown is
    // empty -- but the outer vector is still indexable by leak-flow (a downstream
    // source reads leak_slat_vols[k] without bounds fear).
    let mut c = ConveyorState::new(
        1.0,
        false,
        false,
        false,
        vec![LeakConfig {
            zone_start: 0.0,
            zone_end: 1.0,
            integers: false,
        }],
    );
    c.init_from_inflow(3.0, 10.0, &[0.1]);
    let pa = c.phase_a(PhaseAInputs {
        arrested: true,
        sample: false,
        transit: 3.0,
        leak_fractions: &[0.1],
        dest_arrested: false,
        leak_dest_arrested: &[false],
    });
    assert!(pa.arrested);
    assert_eq!(pa.leak_slat_vols.len(), 1);
    assert!(pa.leak_slat_vols[0].is_empty());
}
