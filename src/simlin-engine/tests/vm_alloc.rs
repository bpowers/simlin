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
use std::sync::Arc;

use simlin_engine::test_common::TestProject;
use simlin_engine::{Project as CompiledProject, Simulation, Vm};

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
    let datamodel = tp.build_datamodel();
    let project = Arc::new(CompiledProject::from(datamodel));
    let sim = Simulation::new(&project, "main").unwrap();
    let compiled = sim.compile().unwrap();
    Vm::new(compiled).unwrap()
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
