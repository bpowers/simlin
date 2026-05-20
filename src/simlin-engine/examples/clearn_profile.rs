// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Standalone profiling harness for the C-LEARN hero model.
//!
//! Times each pipeline stage (parse, compile-via-salsa, VM construction, run)
//! and reports allocation counts / peak live bytes per stage via a counting
//! global allocator. Designed as a focused `perf record` / heaptrack target:
//! set `CLEARN_PROFILE=compile` or `CLEARN_PROFILE=run` and a high iteration
//! count to give an external sampler sustained signal on one stage.
//!
//! Usage:
//!   cargo run --release -p simlin-engine --example clearn_profile
//!   CLEARN_COMPILE_ITERS=20 CLEARN_PROFILE=compile \
//!     perf record -g -- target/release/examples/clearn_profile
//!   CLEARN_RUN_ITERS=200 CLEARN_PROFILE=run \
//!     perf record -g -- target/release/examples/clearn_profile
//!
//! Environment:
//!   CLEARN_MODEL          override the .mdl path
//!   CLEARN_COMPILE_ITERS  extra compile-only iterations (default 0)
//!   CLEARN_RUN_ITERS      extra run-only iterations (default 0)
//!   CLEARN_PROFILE        "compile" | "run" | "both" (default both) -- which
//!                         extra-iteration loop(s) to execute

use std::alloc::{GlobalAlloc, Layout, System as Backing};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use simlin_engine::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};
use simlin_engine::{CompiledSimulation, Vm, open_vensim};

// --- Counting allocator -----------------------------------------------------
//
// Tracks cumulative allocation calls/bytes plus live bytes and a high-water
// mark. compile_project_incremental can fan out across rayon threads, so all
// counters are atomic and the peak is maintained with a CAS loop. The default
// GlobalAlloc::realloc routes through our alloc/dealloc, so realloc is counted
// without an explicit override.

struct Counting;

static COUNTING_ON: AtomicBool = AtomicBool::new(false);
static ALLOC_CALLS: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { Backing.alloc(layout) };
        // Counting is gated so the default run measures true wall-clock without
        // per-allocation atomic overhead. Enable with CLEARN_COUNT_ALLOCS=1 to
        // get allocation counts (at the cost of distorted timing).
        if !p.is_null() && COUNTING_ON.load(Ordering::Relaxed) {
            ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
            let live = LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            let mut peak = PEAK_BYTES.load(Ordering::Relaxed);
            while live > peak {
                match PEAK_BYTES.compare_exchange_weak(
                    peak,
                    live,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(observed) => peak = observed,
                }
            }
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { Backing.dealloc(ptr, layout) };
        if COUNTING_ON.load(Ordering::Relaxed) {
            LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[derive(Clone, Copy)]
struct Snap {
    calls: usize,
    bytes: usize,
    live: usize,
}

fn snap() -> Snap {
    Snap {
        calls: ALLOC_CALLS.load(Ordering::Relaxed),
        bytes: ALLOC_BYTES.load(Ordering::Relaxed),
        live: LIVE_BYTES.load(Ordering::Relaxed),
    }
}

/// Reset the peak high-water mark to the current live bytes so the next phase's
/// peak is measured relative to its own starting point.
fn reset_peak() {
    PEAK_BYTES.store(LIVE_BYTES.load(Ordering::Relaxed), Ordering::Relaxed);
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// Run `f` as a measured phase: report wall time, allocation calls/bytes during
/// the phase, net retained (live) bytes, and peak live bytes reached.
fn phase<T>(name: &str, f: impl FnOnce() -> T) -> T {
    reset_peak();
    let before = snap();
    let t0 = Instant::now();
    let out = f();
    let elapsed = t0.elapsed();
    let after = snap();
    let peak = PEAK_BYTES.load(Ordering::Relaxed);

    let calls = after.calls - before.calls;
    let bytes = after.bytes - before.bytes;
    let retained = after.live as i64 - before.live as i64;

    println!(
        "{name:<22} {:>9.2} ms | allocs {:>10} | alloc'd {:>9.1} MiB | retained {:>+8.1} MiB | peak {:>8.1} MiB",
        elapsed.as_secs_f64() * 1000.0,
        calls,
        mib(bytes),
        retained as f64 / (1024.0 * 1024.0),
        mib(peak),
    );
    out
}

fn model_path() -> String {
    if let Ok(p) = std::env::var("CLEARN_MODEL") {
        return p;
    }
    format!(
        "{}/../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn compile_once(datamodel: &simlin_engine::datamodel::Project) -> CompiledSimulation {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, datamodel, None);
    compile_project_incremental(&db, sync.project, "main").unwrap()
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let path = model_path();
    let compile_iters = env_usize("CLEARN_COMPILE_ITERS", 0);
    let run_iters = env_usize("CLEARN_RUN_ITERS", 0);
    let which = std::env::var("CLEARN_PROFILE").unwrap_or_else(|_| "both".to_string());
    if std::env::var("CLEARN_COUNT_ALLOCS").is_ok_and(|v| v != "0") {
        COUNTING_ON.store(true, Ordering::Relaxed);
    }

    println!("model: {path}");

    let contents = phase("read_file", || {
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"))
    });
    println!(
        "  source: {} bytes, {} lines",
        contents.len(),
        contents.lines().count()
    );

    let datamodel = phase("parse (open_vensim)", || open_vensim(&contents).unwrap());
    let n_models = datamodel.models.len();
    let n_vars: usize = datamodel.models.iter().map(|m| m.variables.len()).sum();
    println!(
        "  models: {n_models}, datamodel variables: {n_vars}, dims: {}",
        datamodel.dimensions.len()
    );

    let compiled = phase("compile (salsa)", || compile_once(&datamodel));
    println!("  n_slots (root): {}", compiled.n_slots());

    let prof = compiled.bytecode_profile();
    println!(
        "  bytecode: {} opcodes ({:.1} KiB @ 8B) = {} flow + {} stock + {} initial ({} initials)",
        prof.total_opcodes,
        (prof.total_opcodes * 8) as f64 / 1024.0,
        prof.flow_opcodes,
        prof.stock_opcodes,
        prof.initial_opcodes,
        prof.n_initials,
    );
    println!(
        "  flow opcodes after 3-address fusion (est): {} -> {} ({:.1}% reduction)",
        prof.flow_opcodes,
        prof.flow_opcodes_after_fusion,
        100.0 * (prof.flow_opcodes - prof.flow_opcodes_after_fusion) as f64
            / prof.flow_opcodes as f64,
    );
    println!(
        "  tables: {} literals, {} GFs / {} points, {} temp slots, {} dims, {} static_views, {} dim_lists, {} names, {} modules",
        prof.total_literals,
        prof.graphical_functions,
        prof.graphical_function_points,
        prof.temp_storage_slots,
        prof.dimensions,
        prof.static_views,
        prof.dim_lists,
        prof.names,
        prof.n_modules,
    );
    let mut hist: Vec<_> = prof.histogram.iter().collect();
    hist.sort_by(|a, b| b.1.cmp(a.1));
    println!("  opcode histogram (top 25 of {}):", prof.histogram.len());
    for (name, count) in hist.iter().take(25) {
        let pct = **count as f64 / prof.total_opcodes as f64 * 100.0;
        println!("    {name:<22} {count:>9}  {pct:>5.1}%");
    }

    let mut vm = phase("Vm::new", || Vm::new(compiled.clone()).unwrap());
    println!("  variables (offsets): {}", vm.names_as_strs().len());

    phase("run_to_end", || vm.run_to_end().unwrap());
    let results = vm.into_results();
    println!(
        "  result slots/step: {}, saved steps: {}",
        results.step_size, results.step_count
    );

    // Extra-iteration loops for external samplers (perf/heaptrack). Kept out of
    // the per-phase accounting above; these print only aggregate timing.
    let do_compile = which == "both" || which == "compile";
    let do_run = which == "both" || which == "run";

    if compile_iters > 0 && do_compile {
        let t0 = Instant::now();
        for _ in 0..compile_iters {
            std::hint::black_box(compile_once(&datamodel));
        }
        let per = t0.elapsed().as_secs_f64() * 1000.0 / compile_iters as f64;
        println!("compile x{compile_iters}: {per:.2} ms/iter");
    }

    if run_iters > 0 && do_run {
        let compiled = compile_once(&datamodel);
        let t0 = Instant::now();
        for _ in 0..run_iters {
            let mut vm = Vm::new(compiled.clone()).unwrap();
            vm.run_to_end().unwrap();
            std::hint::black_box(&vm);
        }
        let per = t0.elapsed().as_secs_f64() * 1000.0 / run_iters as f64;
        println!("run x{run_iters}: {per:.2} ms/iter (incl. Vm::new + clone)");
    }
}
