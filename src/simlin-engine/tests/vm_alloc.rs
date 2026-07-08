// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Integration test that verifies the VM simulation hot path
//! (the per-DT loop inside `run_to`) performs zero heap allocations.
//!
//! Uses a custom global allocator that counts allocations per-thread.
//! Since integration tests compile as their own binary, the
//! #[global_allocator] here does not affect other test binaries.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use simlin_engine::Vm;
use simlin_engine::test_common::TestProject;

// ---------------------------------------------------------------------------
// Per-thread counting allocator
// ---------------------------------------------------------------------------

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static ALLOC_COUNT: Cell<usize> = const { Cell::new(0) };
}

struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        TRACKING.with(|t| {
            if t.get() {
                ALLOC_COUNT.with(|c| c.set(c.get() + 1));
            }
        });
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static A: CountingAllocator = CountingAllocator;

fn start_tracking() {
    ALLOC_COUNT.with(|c| c.set(0));
    TRACKING.with(|t| t.set(true));
}

fn stop_tracking() -> usize {
    TRACKING.with(|t| t.set(false));
    ALLOC_COUNT.with(|c| c.get())
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn build_scalar_model(stop: f64) -> Vm {
    let tp = TestProject::new("alloc_test")
        .with_sim_time(0.0, stop, 1.0)
        .aux("birth_rate", "0.1", None)
        .aux("lifespan", "80", None)
        .aux("initial_pop", "1000 * birth_rate", None)
        .stock("population", "initial_pop", &["births"], &["deaths"], None)
        .flow("births", "population * birth_rate", None)
        .flow("deaths", "population / lifespan", None);
    let compiled = tp.compile_incremental().unwrap();
    Vm::new(compiled).unwrap()
}

/// A scalar uncoupled queue model (arrivals -> queue -> served), built through
/// the special stock-type path so the VM runs the queue pass every step.
fn build_queue_model(stop: f64) -> Vm {
    let xml = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>alloc queue</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>{stop}</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting">
      <eqn>0</eqn>
      <inflow>arrivals</inflow>
      <outflow>into_service</outflow>
      <queue/>
    </stock>
    <flow name="arrivals"><eqn>10</eqn><non_negative/></flow>
    <flow name="into_service"><eqn>0</eqn></flow>
    <stock name="served"><eqn>0</eqn><inflow>into_service</inflow></stock>
  </variables></model>
</xmile>"#
    );
    let project =
        simlin_engine::xmile::project_from_reader(&mut std::io::BufReader::new(xml.as_bytes()))
            .unwrap();
    let main = project.models[0].name.clone();
    simlin_engine::queue_compile::build_vm(&project, &main).unwrap()
}

/// A scalar conveyor with an `even`-placed inflow and two LINEAR leaks -- the
/// GH #879 dominant case: every step inserts O(d) non-zero placement shares
/// (d = transit / dt entry-path slats), each of which used to re-run the
/// share-independent cohort-schedule work with fresh allocations.
fn build_conveyor_model(stop: f64, transit: f64) -> Vm {
    let xml = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>alloc conveyor</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>{stop}</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="belt">
      <eqn>0</eqn>
      <inflow>in_f</inflow>
      <outflow>out_f</outflow>
      <outflow>leak_a</outflow>
      <outflow>leak_b</outflow>
      <conveyor><len>{transit}</len></conveyor>
    </stock>
    <flow name="in_f" isee:spreadflow="even"><eqn>10</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <flow name="leak_a"><eqn>0.05</eqn><leak/></flow>
    <flow name="leak_b"><eqn>0.03</eqn><leak/></flow>
  </variables></model>
</xmile>"#
    );
    let project =
        simlin_engine::xmile::project_from_reader(&mut std::io::BufReader::new(xml.as_bytes()))
            .unwrap();
    let main = project.models[0].name.clone();
    simlin_engine::queue_compile::build_vm(&project, &main).unwrap()
}

/// Run a conveyor model to the end (initials excluded) and return the number of
/// heap allocations the per-DT loop performed.
fn conveyor_run_allocs(stop: f64, transit: f64) -> usize {
    let mut vm = build_conveyor_model(stop, transit);
    vm.run_initials().unwrap();
    start_tracking();
    vm.run_to_end().unwrap();
    stop_tracking()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Verify that the per-DT simulation loop performs zero heap allocations.
#[test]
fn run_to_zero_allocations() {
    let mut vm_short = build_scalar_model(100.0);
    let mut vm_long = build_scalar_model(1000.0);

    // Run initials outside the measured region.
    vm_short.run_initials().unwrap();
    vm_long.run_initials().unwrap();

    start_tracking();
    vm_short.run_to_end().unwrap();
    let allocs_short = stop_tracking();

    start_tracking();
    vm_long.run_to_end().unwrap();
    let allocs_long = stop_tracking();

    assert_eq!(
        allocs_short, allocs_long,
        "allocation count should not scale with step count \
         (short={allocs_short}, long={allocs_long})"
    );
    assert_eq!(
        allocs_short, 0,
        "run_to should perform zero heap allocations, got {allocs_short}"
    );
}

/// The per-DT loop of an (uncoupled) queue model must not allocate per step.
///
/// The queue-conveyor coupling is compile-time constant, so its table is
/// derived once when the plans are attached to the VM (GH #878) -- before
/// that fix `run_coupled_passes` rebuilt it (one heap allocation plus a scan)
/// on EVERY Euler step, only for the no-coupling fast path to discard it.
/// The queue side table itself reaches an allocation steady state (the FIFO's
/// VecDeque retains capacity across the per-step admit/drain), so total
/// allocations are a step-count-independent constant: a 10x longer run must
/// allocate exactly as much as a short one.
#[test]
fn queue_model_allocations_do_not_scale_with_steps() {
    let mut vm_short = build_queue_model(100.0);
    let mut vm_long = build_queue_model(1000.0);

    // Run initials outside the measured region (queue/belt side-table setup
    // legitimately allocates once).
    vm_short.run_initials().unwrap();
    vm_long.run_initials().unwrap();

    start_tracking();
    vm_short.run_to_end().unwrap();
    let allocs_short = stop_tracking();

    start_tracking();
    vm_long.run_to_end().unwrap();
    let allocs_long = stop_tracking();

    assert_eq!(
        allocs_short, allocs_long,
        "queue-model allocation count should not scale with step count \
         (short={allocs_short}, long={allocs_long})"
    );
}

/// The conveyor pass's per-step allocation count must be independent of the
/// entry depth `d` and constant per step (GH #879).
///
/// The dominant hot-loop defect was in `phase_b`'s insert loop: an `even`
/// (or `dist`/`dest`) placement on a linear-leak belt produces one non-zero
/// share per entry-path slat, and each share's cohort-schedule computation
/// re-ran the share-independent `zone_start_retained`/`m_entry` work -- O(d)
/// allocations and O(d^2 * n) work per belt per step. With that work hoisted,
/// the pass still allocates a small CONSTANT number of scratch vectors per
/// step (per-step `PhaseAResult`/shares/fraction buffers).
///
/// `run_to` additionally pays a ONE-TIME per-call cost outside the per-DT
/// loop: the #625 resting-curr pass preview clones the belt side tables
/// (O(d) allocations, proportional to state the belt already holds) so a
/// mid-run read sees pass-computed rates without double-advancing the belts.
/// Every comparison below therefore differences two runs at the same depth,
/// cancelling the run-constant preview cost while still catching any
/// PER-STEP allocation that scales with `d` or with the step count:
///
/// 1. the per-step allocation count does not scale with `d` (equal step
///    deltas at 4x the belt depth yield identical allocation deltas), and
/// 2. the total is exactly linear in the step count (three stops in
///    arithmetic progression yield equal deltas).
#[test]
fn conveyor_pass_allocations_do_not_scale_with_belt_depth_or_steps() {
    // (1) share-count independence: the same step-count delta at different
    // entry depths must allocate identically per step.
    let delta_shallow = conveyor_run_allocs(400.0, 64.0) - conveyor_run_allocs(200.0, 64.0);
    let delta_deep = conveyor_run_allocs(400.0, 256.0) - conveyor_run_allocs(200.0, 256.0);
    assert_eq!(
        delta_shallow, delta_deep,
        "conveyor per-step allocations must not scale with entry depth \
         (d=64: {delta_shallow}, d=256: {delta_deep})"
    );

    // (2) linearity in steps: equal step deltas -> equal allocation deltas.
    let a_short = conveyor_run_allocs(100.0, 256.0);
    let a_mid = conveyor_run_allocs(550.0, 256.0);
    let a_long = conveyor_run_allocs(1000.0, 256.0);
    assert_eq!(
        a_mid - a_short,
        a_long - a_mid,
        "conveyor allocations must be linear in step count \
         (short={a_short}, mid={a_mid}, long={a_long})"
    );
}

/// Same test but exercising the reset+re-run path used by slider interaction.
#[test]
fn reset_and_rerun_zero_allocations() {
    let mut vm = build_scalar_model(100.0);
    vm.run_to_end().unwrap();

    // Warm up the reset+rerun path.
    vm.reset();
    vm.run_to_end().unwrap();

    start_tracking();
    vm.reset();
    vm.run_to_end().unwrap();
    let allocs = stop_tracking();

    assert_eq!(
        allocs, 0,
        "reset+run_to_end should perform zero heap allocations, got {allocs}"
    );
}
